//! One WinMM output, open: the thread that sends what the audio thread
//! hands it, each message at its time, and its close.
//!
//! The audio thread never calls WinMM. It pushes packets into the output's
//! ring, each stamped with when it is due, and sets the output's signal,
//! and the output's own thread takes them in turn: it sleeps until the next
//! is due or the signal wakes it, then sends what is due. A SysEx is joined
//! from its packets and sent whole as one long message, which the thread
//! waits on while the driver sends it, so a slow port holds up only its own
//! output.

use std::fmt;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use hapto_ump::{Bytes, Event, MessageType, SysExJoin};
use windows::Win32::Media::Audio::{
    CALLBACK_EVENT, HMIDIOUT, MHDR_DONE, MIDIERR_NODEVICE, MIDIHDR, midiOutClose, midiOutLongMsg,
    midiOutOpen, midiOutPrepareHeader, midiOutReset, midiOutShortMsg, midiOutUnprepareHeader,
};
use windows::Win32::Media::{MMSYSERR_NODRIVER, MMSYSERR_NOERROR};
use windows::core::PSTR;

use super::signal::{EventHandle, Signal, Timer};
use super::{CLOSE_TRIES, UNPREPARE_TRIES, Waits, check};
use crate::Instant;
use crate::error::OpenError;
use crate::id::{Device, Direction};
use crate::ring::Consumer;

/// How long the output's thread waits at a time for the driver to send a
/// SysEx.
const SYSEX_WAIT_MS: u32 = 10;
/// How long a close waits, after its reset, for the driver to hand a SysEx
/// back, before it leaves the SysEx's room to the driver.
const RESET_WAIT: std::time::Duration = std::time::Duration::from_secs(1);
/// The longest SysEx an output sends whole; a longer one is dropped and
/// counted.
pub(crate) const SYSEX_BYTES: usize = 64 * 1024;

/// A WinMM output handle, which any thread may use.
#[derive(Clone, Copy)]
struct Handle(HMIDIOUT);

// SAFETY: a WinMM handle is a token the system resolves, valid on any
// thread; the output closes it only after its thread is done with it.
unsafe impl Send for Handle {}

/// What the output shares with its thread.
struct Shared {
    /// Tells the thread to end, resetting the device if a SysEx is under
    /// way.
    stop: AtomicBool,
    /// Set when a send found the device gone: it left.
    left: AtomicBool,
    /// Set by WinMM as it finishes with a SysEx: the output's thread waits
    /// on it.
    done: EventHandle,
}

/// A SysEx header, aligned for its pointers and its flags: WinMM packs its
/// structs.
#[repr(C, align(8))]
struct Header(MIDIHDR);

/// A SysEx being joined from its packets, and the header WinMM sends it by.
struct Sysex {
    header: Box<Header>,
    /// The SysEx to send, copied from the join: the driver reads it here.
    bytes: Vec<u8>,
    /// Joins every packet the ring hands over, so a message on a SysEx's
    /// group ends it.
    join: SysExJoin,
    /// The packets the ring had dropped as the SysEx under way began, those
    /// this output counted itself left out: one more since lost a packet of
    /// it.
    ring_dropped_at_start: u64,
    /// The drops this output counted on the ring itself.
    counted: u64,
}

/// What one packet came to.
#[derive(Debug, PartialEq, Eq)]
struct Joined {
    /// Whether a SysEx is whole in the buffer, to send.
    whole: bool,
    /// How many messages it dropped, which the caller counts on the ring.
    dropped: u32,
}

impl Sysex {
    fn new() -> Sysex {
        Sysex {
            header: Box::new(Header(MIDIHDR::default())),
            bytes: Vec::with_capacity(SYSEX_BYTES),
            join: SysExJoin::with_room(SYSEX_BYTES.saturating_sub(2)),
            ring_dropped_at_start: 0,
            counted: 0,
        }
    }

    /// Reads one packet, `ring_dropped` the ring's dropped count now. A
    /// SysEx whole goes into the buffer, unless the ring dropped a packet
    /// while it came, maybe one of its.
    fn join(&mut self, event: &Event, ring_dropped: u64) -> Joined {
        let from_ring = ring_dropped.saturating_sub(self.counted);
        if let Some(packet) = event.sysex7()
            && packet.form.begins()
            && self
                .join
                .under_way()
                .is_none_or(|group| group == packet.group)
        {
            self.ring_dropped_at_start = from_ring;
        }
        let joined = self.join.push(event);
        let mut dropped = joined.dropped;
        let mut whole = false;
        if let Some(message) = joined.whole {
            let framed = message.framed();
            // A buffer a hung driver kept has no room left: nothing fits.
            if from_ring != self.ring_dropped_at_start || framed.len() > self.bytes.capacity() {
                dropped = dropped.saturating_add(1);
            } else {
                self.bytes.clear();
                self.bytes.extend_from_slice(framed);
                whole = true;
            }
        }
        self.counted = self.counted.saturating_add(u64::from(dropped));
        Joined { whole, dropped }
    }

    /// Sends the SysEx joined, and waits while the driver sends it: until
    /// it is done, or a close resets the device, which hands it back.
    /// Answers whether it went.
    fn send(&mut self, handle: Handle, shared: &Shared) -> bool {
        let size = size_of::<MIDIHDR>() as u32;
        let length = u32::try_from(self.bytes.len()).unwrap_or(u32::MAX);
        self.header.0 = MIDIHDR {
            lpData: PSTR(self.bytes.as_mut_ptr()),
            dwBufferLength: length,
            dwBytesRecorded: length,
            ..MIDIHDR::default()
        };
        let header = &raw mut self.header.0;
        // SAFETY: the open handle, and the header, which points at the
        // joined bytes; neither moves until it is unprepared below.
        let prepared = unsafe { midiOutPrepareHeader(handle.0, header, size) };
        if prepared != MMSYSERR_NOERROR {
            self.bytes.clear();
            return false;
        }
        // SAFETY: as above, the header prepared.
        let sent = unsafe { midiOutLongMsg(handle.0, header, size) };
        if sent == MMSYSERR_NOERROR {
            let mut reset: Option<Instant> = None;
            while !done(header) {
                if reset.is_none() && shared.stop.load(Ordering::Acquire) {
                    // SAFETY: the open handle; the reset hands the header
                    // back marked done.
                    unsafe { midiOutReset(handle.0) };
                    reset = Some(Instant::now());
                }
                if reset.is_some_and(|at| Instant::now().since(at) > RESET_WAIT) {
                    // A driver that never hands it back may still read the
                    // header and its bytes: they are left to it, and the
                    // device is gone. The WDM driver never returns from the
                    // send at all for a device unplugged mid-SysEx, as the
                    // open note on `Outputs` says, so this does not save it.
                    let header =
                        std::mem::replace(&mut self.header, Box::new(Header(MIDIHDR::default())));
                    Box::leak(header);
                    std::mem::take(&mut self.bytes).leak();
                    shared.left.store(true, Ordering::Release);
                    return false;
                }
                // SAFETY: the shared event, which the device sets as it
                // finishes with the header.
                shared.done.wait(SYSEX_WAIT_MS);
            }
        } else {
            note_gone(shared, sent);
        }
        // SAFETY: the open handle and the header, done with.
        Waits(UNPREPARE_TRIES).retry(|| unsafe { midiOutUnprepareHeader(handle.0, header, size) });
        self.bytes.clear();
        sent == MMSYSERR_NOERROR
    }
}

/// Whether the driver is done with the SysEx `header` holds.
fn done(header: *const MIDIHDR) -> bool {
    // SAFETY: the output's own header, alive while it sends. Its flags sit
    // on a four-byte boundary of a header aligned to eight, and the driver
    // writes them as it finishes, so each look reads them anew.
    let flags = unsafe { ptr::read_volatile(&raw const (*header).dwFlags) };
    flags & MHDR_DONE != 0
}

/// Marks the output left when `code` says its device is gone.
fn note_gone(shared: &Shared, code: u32) {
    if matches!(code, MMSYSERR_NODRIVER | MIDIERR_NODEVICE) {
        shared.left.store(true, Ordering::Release);
    }
}

/// An output, sending from its ring while open. It closes as it drops.
pub(crate) struct Output {
    handle: Handle,
    /// `None` once closed.
    shared: Option<Arc<Shared>>,
    thread: Option<JoinHandle<Consumer>>,
    /// What wakes the thread: the audio thread sets it as it pushes.
    signal: Arc<Signal>,
}

impl fmt::Debug for Output {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Output")
            .field("open", &self.shared.is_some())
            .finish_non_exhaustive()
    }
}

impl Output {
    /// Opens `device` to send what `consumer`'s ring holds, its thread
    /// woken by `signal`. On a refusal the consumer comes back, unless the
    /// thread that would send could not start, which took it.
    pub(crate) fn open(
        device: &Device,
        consumer: Consumer,
        signal: Arc<Signal>,
    ) -> Result<Output, (OpenError, Option<Consumer>)> {
        let Some(index) = super::index_of(Direction::Output, device.id()) else {
            return Err((OpenError::Gone, Some(consumer)));
        };
        let Some(done) = EventHandle::new() else {
            return Err((OpenError::NoMemory, Some(consumer)));
        };
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            left: AtomicBool::new(false),
            done,
        });
        let mut handle = HMIDIOUT::default();
        // SAFETY: the handle is written by the call. WinMM sets the event
        // as it finishes with a SysEx; the event outlives the device, as
        // `Shared` holds it until the close.
        let opened = unsafe {
            midiOutOpen(
                &raw mut handle,
                index,
                Some(shared.done.raw().0 as usize),
                None,
                CALLBACK_EVENT,
            )
        };
        if let Err(why) = check(opened) {
            return Err((why, Some(consumer)));
        }
        let handle = Handle(handle);
        let (thread_shared, thread_signal) = (Arc::clone(&shared), Arc::clone(&signal));
        let started = thread::Builder::new()
            .name("hapto midi out".to_owned())
            .spawn(move || send(handle, &thread_shared, &thread_signal, consumer));
        let mut output = Output {
            handle,
            shared: Some(shared),
            thread: None,
            signal,
        };
        match started {
            Ok(thread) => {
                output.thread = Some(thread);
                Ok(output)
            }
            Err(_) => {
                output.shut();
                Err((OpenError::NoMemory, None))
            }
        }
    }

    /// Whether the device left while open.
    pub(crate) fn has_left(&self) -> bool {
        self.shared
            .as_ref()
            .is_some_and(|shared| shared.left.load(Ordering::Acquire))
    }

    /// Closes the device and gives its ring's consumer back, with what it
    /// had not sent dropped.
    pub(crate) fn close(mut self) -> Option<Consumer> {
        self.shut()
    }

    /// Stops the thread, then resets and closes the device. If the device
    /// does not close, the event it sets is leaked rather than closed
    /// under the driver.
    fn shut(&mut self) -> Option<Consumer> {
        let shared = self.shared.take()?;
        shared.stop.store(true, Ordering::Release);
        // Wakes the thread, waiting for a packet or on a SysEx.
        self.signal.set();
        // SAFETY: the shared event, alive with it.
        shared.done.set();
        let consumer = self.thread.take().and_then(|thread| thread.join().ok());
        let handle = self.handle.0;
        // SAFETY: the open handle, which the stopped thread no longer uses.
        // The reset turns off any note the device was told to play.
        unsafe { midiOutReset(handle) };
        // SAFETY: the open handle, closed once.
        if !Waits(CLOSE_TRIES).retry(|| unsafe { midiOutClose(handle) }) {
            std::mem::forget(shared);
        }
        consumer
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.shut();
    }
}

/// The output's thread: it sends each packet once its time comes, until
/// the output stops it, and hands the ring back. It waits for `signal`
/// while nothing waits to be sent, and on its own timer for a packet due
/// later, which the signal also ends.
fn send(handle: Handle, shared: &Shared, signal: &Signal, consumer: Consumer) -> Consumer {
    // The ring is read from here until the thread ends however it ends, so
    // the writer pushes into no ring nobody reads.
    let mut consumer = Reading::new(consumer);
    let timer = Timer::new();
    let mut sysex = Sysex::new();
    while !shared.stop.load(Ordering::Acquire) {
        let Some((due, words)) = consumer.peek() else {
            timer.wait(signal, None);
            continue;
        };
        let now = Instant::now();
        if due > now {
            timer.wait(signal, Some(due.since(now)));
            continue;
        }
        consumer.pop();
        // The writer pushes only what MIDI 1.0's bytes carry.
        let Some(bytes) = Bytes::of(words) else {
            continue;
        };
        let event = Event::new(0, words);
        let joined = sysex.join(&event, consumer.dropped());
        for _ in 0..joined.dropped {
            consumer.count_dropped();
        }
        if joined.whole && !sysex.send(handle, shared) {
            consumer.count_dropped();
        }
        if event.message_type() != MessageType::SYSEX7 {
            short(handle, shared, &bytes);
        }
    }
    consumer.into_inner()
}

/// A consumer its output's thread reads from, marked read while it lives.
struct Reading(Option<Consumer>);

impl Reading {
    fn new(consumer: Consumer) -> Reading {
        consumer.set_reading(true);
        Reading(Some(consumer))
    }

    /// The consumer, no longer read.
    fn into_inner(mut self) -> Consumer {
        let consumer = self.0.take().expect("held until here");
        consumer.set_reading(false);
        consumer
    }
}

impl std::ops::Deref for Reading {
    type Target = Consumer;

    fn deref(&self) -> &Consumer {
        self.0.as_ref().expect("held until into_inner")
    }
}

impl std::ops::DerefMut for Reading {
    fn deref_mut(&mut self) -> &mut Consumer {
        self.0.as_mut().expect("held until into_inner")
    }
}

impl Drop for Reading {
    fn drop(&mut self) {
        // A thread that ends in a panic: the ring is read no more.
        if let Some(consumer) = &self.0 {
            consumer.set_reading(false);
        }
    }
}

/// Sends one message of up to three bytes, packed as WinMM takes it: the
/// status in the low byte, then its data.
fn short(handle: Handle, shared: &Shared, bytes: &[u8]) {
    // SAFETY: the open handle.
    let sent = unsafe { midiOutShortMsg(handle.0, pack(bytes)) };
    note_gone(shared, sent);
}

/// Up to three bytes of a message, packed as WinMM takes them: the first,
/// the status, in the low byte.
fn pack(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .take(3)
        .rev()
        .fold(0_u32, |packed, &byte| packed << 8 | u32::from(byte))
}

#[cfg(test)]
mod tests;
