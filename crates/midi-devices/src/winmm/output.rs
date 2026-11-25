//! One open WinMM output, with the thread that sends what the audio thread
//! hands it, each message at its time, and its close.
//!
//! The audio thread never calls WinMM. It pushes packets, each stamped with
//! its due time, into the output's ring and sets the output's signal. The
//! output's thread sleeps until the next packet is due or the signal wakes
//! it, then sends what is due.
//!
//! A SysEx is joined from its packets and sent whole as one long message.
//! The thread waits while the driver sends it, so a slow port holds up only
//! its own output.

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
/// How long a close waits after its reset for the driver to hand a SysEx
/// back, before leaving the SysEx's buffer to the driver.
const RESET_WAIT: std::time::Duration = std::time::Duration::from_secs(1);
/// The longest SysEx that an output sends whole, rather than dropping and
/// counting it.
pub(crate) const SYSEX_BYTES: usize = 64 * 1024;

/// A WinMM output handle, which any thread can use.
#[derive(Clone, Copy)]
struct Handle(HMIDIOUT);

// SAFETY: a WinMM handle is a token that the system resolves, so it is valid
// on any thread. The output closes it only after its thread is done with it.
unsafe impl Send for Handle {}

/// What the output shares with its thread.
struct Shared {
    /// Tells the thread to end, resetting the device if a SysEx is in
    /// progress.
    stop: AtomicBool,
    /// Set when a send found the device gone, because it left.
    left: AtomicBool,
    /// The event that the output's thread waits on, which WinMM sets when it
    /// finishes with a SysEx.
    done: EventHandle,
}

/// A SysEx header, aligned for its pointers and its flags because WinMM
/// packs its structs.
#[repr(C, align(8))]
struct Header(MIDIHDR);

/// A SysEx being joined from its packets, and the header that WinMM sends it
/// with.
struct Sysex {
    header: Box<Header>,
    /// The SysEx to send, copied from the join, where the driver reads it.
    bytes: Vec<u8>,
    /// Joins every packet that the ring hands over, so a message on a
    /// SysEx's group ends it.
    join: SysExJoin,
    /// The ring's dropped count, less this output's own drops, when the
    /// SysEx in progress began, so that a later drop counts as a lost packet.
    ring_dropped_at_start: u64,
    /// The drops that this output counted on the ring itself.
    counted: u64,
}

/// The result of joining one packet.
#[derive(Debug, PartialEq, Eq)]
struct Joined {
    /// Whether a whole SysEx is in the buffer to send.
    whole: bool,
    /// How many messages it dropped, for the caller to count on the ring.
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

    /// Reads one packet, with `ring_dropped` as the ring's dropped count.
    ///
    /// A whole SysEx goes into the buffer, unless the ring dropped a packet,
    /// maybe one of its own, while it arrived.
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
            // A buffer that a hung driver kept has no capacity left, so
            // nothing fits.
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

    /// Sends the joined SysEx, waits until the driver is done or a close
    /// resets the device, and returns whether it was sent.
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
        // SAFETY: the handle is open, and the header points at the joined
        // bytes. Neither moves until it is unprepared below.
        let prepared = unsafe { midiOutPrepareHeader(handle.0, header, size) };
        if prepared != MMSYSERR_NOERROR {
            self.bytes.clear();
            return false;
        }
        // SAFETY: as above, now prepared.
        let sent = unsafe { midiOutLongMsg(handle.0, header, size) };
        if sent == MMSYSERR_NOERROR {
            let mut reset: Option<Instant> = None;
            while !done(header) {
                if reset.is_none() && shared.stop.load(Ordering::Acquire) {
                    // SAFETY: the handle is open. The reset hands the header
                    // back, marked done.
                    unsafe { midiOutReset(handle.0) };
                    reset = Some(Instant::now());
                }
                if reset.is_some_and(|at| Instant::now().since(at) > RESET_WAIT) {
                    // A driver that never hands the header back might still
                    // read it and its bytes, so they are leaked to it, and
                    // the device is marked as left. This doesn't help for a
                    // device unplugged during a SysEx, because the WDM driver
                    // then never returns from the send, as the open note on
                    // `Outputs` says.
                    let header =
                        std::mem::replace(&mut self.header, Box::new(Header(MIDIHDR::default())));
                    Box::leak(header);
                    std::mem::take(&mut self.bytes).leak();
                    shared.left.store(true, Ordering::Release);
                    return false;
                }
                // SAFETY: the shared event, which the device sets when it
                // finishes with the header.
                shared.done.wait(SYSEX_WAIT_MS);
            }
        } else {
            note_gone(shared, sent);
        }
        // SAFETY: the handle is open, and the driver is done with the
        // header.
        Waits(UNPREPARE_TRIES).retry(|| unsafe { midiOutUnprepareHeader(handle.0, header, size) });
        self.bytes.clear();
        sent == MMSYSERR_NOERROR
    }
}

/// Returns whether the driver is done with the SysEx that `header` holds.
fn done(header: *const MIDIHDR) -> bool {
    // SAFETY: the output's own header, alive while it sends. Its flags sit on
    // a four-byte boundary of a header aligned to eight bytes. The driver
    // writes them when it finishes, so each check reads them again.
    let flags = unsafe { ptr::read_volatile(&raw const (*header).dwFlags) };
    flags & MHDR_DONE != 0
}

/// Marks the output as left when `code` means that its device is gone.
fn note_gone(shared: &Shared, code: u32) {
    if matches!(code, MMSYSERR_NODRIVER | MIDIERR_NODEVICE) {
        shared.left.store(true, Ordering::Release);
    }
}

/// An output, which sends from its ring while open and closes when it drops.
pub(crate) struct Output {
    handle: Handle,
    /// `None` once closed.
    shared: Option<Arc<Shared>>,
    thread: Option<JoinHandle<Consumer>>,
    /// The signal that wakes the thread, which the audio thread sets when it
    /// pushes.
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
    /// Opens `device` to send what `consumer`'s ring holds, with `signal` to
    /// wake its thread.
    ///
    /// On an error, the consumer comes back, unless the thread failed to
    /// start and took it.
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
        // SAFETY: the call writes the handle. WinMM sets the event when it
        // finishes with a SysEx, and the event outlives the device, because
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

    /// Returns whether the device left while open.
    pub(crate) fn has_left(&self) -> bool {
        self.shared
            .as_ref()
            .is_some_and(|shared| shared.left.load(Ordering::Acquire))
    }

    /// Closes the device, dropping what it hadn't sent, and returns its
    /// ring's consumer.
    pub(crate) fn close(mut self) -> Option<Consumer> {
        self.shut()
    }

    /// Stops the thread, then resets and closes the device.
    ///
    /// If the device doesn't close, the event that it sets is leaked, because
    /// the driver might still set it.
    fn shut(&mut self) -> Option<Consumer> {
        let shared = self.shared.take()?;
        shared.stop.store(true, Ordering::Release);
        // Wakes the thread, waiting for a packet or on a SysEx.
        self.signal.set();
        // SAFETY: the shared event, alive with `shared`.
        shared.done.set();
        let consumer = self.thread.take().and_then(|thread| thread.join().ok());
        let handle = self.handle.0;
        // SAFETY: the handle is open, and the stopped thread no longer uses
        // it. The reset turns off any playing note.
        unsafe { midiOutReset(handle) };
        // SAFETY: the handle is open and closed once.
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

/// Runs the output's thread, which sends each packet when it is due, and
/// returns the ring's consumer when the output stops it.
///
/// The thread waits for `signal` while nothing is waiting to be sent, and on
/// its own timer for a packet due later, which the signal also ends.
fn send(handle: Handle, shared: &Shared, signal: &Signal, consumer: Consumer) -> Consumer {
    // The ring is marked as read until the thread ends, however it ends, so
    // the writer never pushes into a ring that nobody reads.
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
        // The writer pushes only what MIDI 1.0 bytes can carry.
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

/// A consumer that its output's thread reads from, marked as read while it
/// lives.
struct Reading(Option<Consumer>);

impl Reading {
    fn new(consumer: Consumer) -> Reading {
        consumer.set_reading(true);
        Reading(Some(consumer))
    }

    /// Returns the consumer, marked as no longer read.
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
        // The thread ended in a panic, so the ring is no longer read.
        if let Some(consumer) = &self.0 {
            consumer.set_reading(false);
        }
    }
}

/// Sends one message of up to three bytes, packed by `pack`.
fn short(handle: Handle, shared: &Shared, bytes: &[u8]) {
    // SAFETY: the handle is open.
    let sent = unsafe { midiOutShortMsg(handle.0, pack(bytes)) };
    note_gone(shared, sent);
}

/// Packs up to three bytes of a message as WinMM takes them, with the first,
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
