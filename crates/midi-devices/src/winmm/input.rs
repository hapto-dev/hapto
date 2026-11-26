//! One open WinMM input, with its callback, the thread that hands its SysEx
//! buffers back, and its close.
//!
//! WinMM calls the callback on its own thread and documents that it may not
//! call any multimedia function. So it only reads messages into the input's
//! ring and marks each SysEx buffer that it is done with, and the input's
//! own thread hands those back.

use std::fmt;
use std::panic::{self, AssertUnwindSafe};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use hapto_ump::{ByteStream, Bytes};
use windows::Win32::Media::Audio::{
    CALLBACK_FUNCTION, HMIDIIN, MIDIHDR, midiInAddBuffer, midiInClose, midiInOpen,
    midiInPrepareHeader, midiInReset, midiInStart, midiInStop, midiInUnprepareHeader,
};
use windows::Win32::Media::{
    MM_MIM_CLOSE, MM_MIM_DATA, MM_MIM_ERROR, MM_MIM_LONGDATA, MM_MIM_LONGERROR, MM_MIM_MOREDATA,
};
use windows::Win32::System::Threading::INFINITE;
use windows::core::PSTR;

use super::signal::EventHandle;
use super::{CLOSE_TRIES, UNPREPARE_TRIES, Waits, check};
use crate::Instant;
use crate::error::OpenError;
use crate::id::{Device, Direction};
use crate::ring::Producer;

/// How many SysEx buffers an input keeps with the driver, and how long each
/// is.
///
/// A long SysEx fills them in turn, and each comes back to be read.
const BUFFERS: usize = 16;
const BUFFER_BYTES: usize = 1024;
// A buffer's packets fit in the room that the reader keeps for a `BURST`.
const _: () = assert!(BUFFER_BYTES.div_ceil(6) < crate::inputs::BURST);

/// One SysEx buffer, with the header that WinMM reads and writes, and its
/// bytes.
///
/// It's aligned for the header's pointers because WinMM packs its structs.
#[repr(C, align(8))]
struct Buffer {
    header: MIDIHDR,
    bytes: [u8; BUFFER_BYTES],
}

/// A WinMM input handle, which any thread can use.
#[derive(Clone, Copy)]
struct Handle(HMIDIIN);

// SAFETY: a WinMM handle is a token that the system resolves, so it is valid
// on any thread. The input closes it only after the threads that use it are
// done.
unsafe impl Send for Handle {}

/// The input's buffers, which WinMM writes into while it holds them.
///
/// They are reached only through this pointer, never borrowed, and freed
/// only after the device closes.
#[derive(Clone, Copy)]
struct Buffers(*mut Buffer);

// SAFETY: WinMM owns the buffers while it holds them, and they are reached
// only through raw pointers. The thread that hands them back stops before
// the close frees them.
unsafe impl Send for Buffers {}

impl Buffers {
    fn new() -> Buffers {
        let buffers: Box<[Buffer]> = (0..BUFFERS)
            .map(|_| Buffer {
                header: MIDIHDR::default(),
                bytes: [0; BUFFER_BYTES],
            })
            .collect();
        Buffers(Box::into_raw(buffers).cast::<Buffer>())
    }

    /// Returns the header of buffer `index`, which must be below
    /// [`BUFFERS`].
    fn header(self, index: usize) -> *mut MIDIHDR {
        debug_assert!(index < BUFFERS);
        // SAFETY: `index` is within the buffers, which are alive until
        // `free`.
        unsafe { &raw mut (*self.0.add(index)).header }
    }

    /// Points buffer `index`'s header at its bytes, with nothing recorded.
    fn reset(self, index: usize) {
        // SAFETY: as for `header`. WinMM doesn't hold the buffer yet, so
        // nothing else writes it.
        unsafe {
            let buffer = self.0.add(index);
            let header = MIDIHDR {
                lpData: PSTR((&raw mut (*buffer).bytes).cast::<u8>()),
                dwBufferLength: BUFFER_BYTES as u32,
                dwUser: index,
                ..MIDIHDR::default()
            };
            ptr::write(&raw mut (*buffer).header, header);
        }
    }

    /// Frees the buffers.
    ///
    /// # Safety
    ///
    /// The device must be closed, so WinMM has released every buffer.
    unsafe fn free(self) {
        // SAFETY: the slice that `new` made, taken back once. The caller
        // guarantees that nothing else reaches it.
        drop(unsafe { Box::from_raw(ptr::slice_from_raw_parts_mut(self.0, BUFFERS)) });
    }
}

/// What the callback reads into.
struct Reading {
    bytes: ByteStream,
    producer: Producer,
}

/// What the input shares with its callback and its thread.
///
/// It outlives every callback, because it's released only after the device
/// closes, and leaked if it doesn't.
struct Shared {
    /// Set when the close begins. Then the callback reads nothing and hands
    /// no buffer back, and the input's thread ends.
    closing: AtomicBool,
    /// Set when the device closed without the input closing it, because it
    /// left.
    left: AtomicBool,
    /// When the device started, in the clock's nanoseconds.
    ///
    /// WinMM stamps messages in milliseconds since then.
    started: AtomicU64,
    /// Locked only by the callback while the device runs.
    reading: Mutex<Reading>,
    /// Which buffers came back, for the input's thread to hand back.
    returned: [AtomicBool; BUFFERS],
    /// The event that the input's thread waits on, which the close sets, and
    /// the callback sets when a buffer comes back, as WinMM allows.
    wake: EventHandle,
    /// How many callbacks are running, which the close waits for before it
    /// frees what they read.
    in_callback: AtomicU32,
}

impl Shared {
    fn new(producer: Producer, wake: EventHandle) -> Shared {
        Shared {
            closing: AtomicBool::new(false),
            left: AtomicBool::new(false),
            started: AtomicU64::new(Instant::now().nanos()),
            reading: Mutex::new(Reading {
                // Every input on group 1, merged as a host merges them.
                bytes: ByteStream::new(0),
                producer,
            }),
            returned: std::array::from_fn(|_| AtomicBool::new(false)),
            wake,
            in_callback: AtomicU32::new(0),
        }
    }

    /// Handles one WinMM message and its parameters, `first` and `second`.
    fn message(&self, message: u32, first: usize, second: usize) {
        // Sequentially consistent with the close's store and its wait on
        // `in_callback`, so a callback either sees the close or is counted
        // before the close waits.
        if self.closing.load(Ordering::SeqCst) {
            return;
        }
        let at = self.stamp(second);
        match message {
            MM_MIM_DATA | MM_MIM_MOREDATA => {
                // A whole message, with running status filled in and the
                // status in the low byte.
                let packed = (first as u32).to_le_bytes();
                // An undefined status alone, which the stream drops.
                let length = Bytes::length_of(packed[0]).unwrap_or(1);
                self.read(at, packed.get(..length).unwrap_or(&[]));
            }
            MM_MIM_LONGDATA | MM_MIM_LONGERROR => {
                let header = first as *const MIDIHDR;
                // SAFETY: WinMM hands back one of the input's headers, which
                // live until the close frees them after the device closes.
                // It's read by value because the struct is packed.
                let header = unsafe { ptr::read_unaligned(header) };
                if message == MM_MIM_LONGDATA {
                    let recorded = (header.dwBytesRecorded as usize).min(BUFFER_BYTES);
                    // SAFETY: the driver wrote `recorded` bytes into the
                    // buffer's bytes, and is done with it until it is handed
                    // back.
                    let bytes = unsafe { std::slice::from_raw_parts(header.lpData.0, recorded) };
                    self.read(at, bytes);
                } else {
                    // A SysEx that the driver couldn't read whole ends where
                    // it broke off, so the reader isn't left holding it.
                    self.cut(at);
                }
                self.hand_back(header.dwUser);
            }
            MM_MIM_ERROR => self.cut(at),
            MM_MIM_CLOSE => self.left.store(true, Ordering::Release),
            _ => {}
        }
    }

    /// Returns when a message that WinMM stamped `millis` arrived.
    fn stamp(&self, millis: usize) -> Instant {
        let started = Instant::from_nanos(self.started.load(Ordering::Acquire));
        started.after(Duration::from_millis(
            u64::try_from(millis).unwrap_or(u64::MAX),
        ))
    }

    fn read(&self, at: Instant, bytes: &[u8]) {
        let mut reading = self.lock();
        let Reading {
            bytes: stream,
            producer,
        } = &mut *reading;
        stream.read(bytes, |words| producer.push(at, words));
    }

    /// Ends a SysEx in progress at `at`, with the stream's `cut`.
    fn cut(&self, at: Instant) {
        let mut reading = self.lock();
        let Reading {
            bytes: stream,
            producer,
        } = &mut *reading;
        stream.cut(|words| producer.push(at, words));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Reading> {
        self.reading.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Marks buffer `index` as back from the driver, and wakes the thread
    /// that hands it back.
    fn hand_back(&self, index: usize) {
        if let Some(flag) = self.returned.get(index) {
            flag.store(true, Ordering::Release);
            self.wake.set();
        }
    }

    /// Returns the ring's producer, once nothing else holds the shared state.
    fn into_producer(this: Arc<Shared>) -> Option<Producer> {
        let shared = Arc::try_unwrap(this).ok()?;
        let Reading {
            bytes: mut stream,
            mut producer,
        } = shared
            .reading
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        // A SysEx that the close cut short ends there, so the reader doesn't
        // hold it, and the real-time messages behind it, forever.
        let now = Instant::now();
        stream.cut(|words| producer.push(now, words));
        Some(producer)
    }
}

/// Reads, pushes and marks for WinMM without calling it, and catches any
/// panic so that none crosses into the system.
unsafe extern "system" fn callback(
    _handle: HMIDIIN,
    message: u32,
    instance: usize,
    first: usize,
    second: usize,
) {
    // SAFETY: `instance` is the input's shared state, which outlives every
    // callback of its device, as `Shared` says.
    let shared = unsafe { &*(instance as *const Shared) };
    shared.in_callback.fetch_add(1, Ordering::SeqCst);
    let _ = panic::catch_unwind(AssertUnwindSafe(|| shared.message(message, first, second)));
    shared.in_callback.fetch_sub(1, Ordering::SeqCst);
}

/// An input, which reads into its ring while open and closes when it drops.
pub(crate) struct Input {
    handle: Handle,
    /// `None` once closed.
    shared: Option<Arc<Shared>>,
    buffers: Option<Buffers>,
    thread: Option<JoinHandle<()>>,
}

impl fmt::Debug for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Input")
            .field("open", &self.shared.is_some())
            .finish_non_exhaustive()
    }
}

impl Input {
    /// Opens `device` to read into `producer`'s ring.
    ///
    /// On an error, the producer comes back, unless the system wouldn't close
    /// the half-opened device.
    pub(crate) fn open(
        device: &Device,
        producer: Producer,
    ) -> Result<Input, (OpenError, Option<Producer>)> {
        let Some(index) = super::index_of(Direction::Input, device.id()) else {
            return Err((OpenError::Gone, Some(producer)));
        };
        let Some(wake) = EventHandle::new() else {
            return Err((OpenError::NoMemory, Some(producer)));
        };
        let shared = Arc::new(Shared::new(producer, wake));
        let mut handle = HMIDIIN::default();
        let callback = callback as unsafe extern "system" fn(HMIDIIN, u32, usize, usize, usize);
        // SAFETY: the call writes the handle. The callback is this module's,
        // and its instance is the shared state, which outlives every
        // callback, as `Shared` says.
        let opened = unsafe {
            midiInOpen(
                &raw mut handle,
                index,
                Some(callback as usize),
                Some(Arc::as_ptr(&shared) as usize),
                CALLBACK_FUNCTION,
            )
        };
        if let Err(why) = check(opened) {
            return Err((why, Shared::into_producer(shared)));
        }
        let mut input = Input {
            handle: Handle(handle),
            shared: Some(shared),
            buffers: Some(Buffers::new()),
            thread: None,
        };
        match input.start() {
            Ok(()) => Ok(input),
            Err(why) => Err((why, input.shut())),
        }
    }

    /// Hands WinMM the buffers, starts the thread that hands them back,
    /// and starts the device.
    fn start(&mut self) -> Result<(), OpenError> {
        let (Some(shared), Some(buffers)) = (&self.shared, self.buffers) else {
            return Err(OpenError::Gone);
        };
        let size = size_of::<MIDIHDR>() as u32;
        for index in 0..BUFFERS {
            buffers.reset(index);
            // SAFETY: the handle is open, and WinMM doesn't hold this
            // buffer's header yet.
            check(unsafe { midiInPrepareHeader(self.handle.0, buffers.header(index), size) })?;
            // SAFETY: the same header, now prepared.
            check(unsafe { midiInAddBuffer(self.handle.0, buffers.header(index), size) })?;
        }
        let (handle, thread_shared) = (self.handle, Arc::clone(shared));
        let thread = thread::Builder::new()
            .name("hapto midi in".to_owned())
            .spawn(move || hand_back(handle, buffers, &thread_shared))
            .map_err(|_| OpenError::NoMemory)?;
        self.thread = Some(thread);
        shared
            .started
            .store(Instant::now().nanos(), Ordering::Release);
        // SAFETY: the handle is open, and WinMM holds its buffers.
        check(unsafe { midiInStart(self.handle.0) })
    }

    /// Returns whether the device left while open.
    pub(crate) fn has_left(&self) -> bool {
        self.shared
            .as_ref()
            .is_some_and(|shared| shared.left.load(Ordering::Acquire))
    }

    /// Closes the device and returns its ring's producer, unless the system
    /// wouldn't close it.
    pub(crate) fn close(mut self) -> Option<Producer> {
        self.shut()
    }

    /// Stops the thread, then stops, resets and closes the device, taking its
    /// buffers back.
    ///
    /// If the device doesn't close, the buffers and the shared state are
    /// leaked, because the driver might still use them.
    fn shut(&mut self) -> Option<Producer> {
        let shared = self.shared.take()?;
        shared.closing.store(true, Ordering::SeqCst);
        let left = shared.left.load(Ordering::Acquire);
        shared.wake.set();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let handle = self.handle.0;
        // SAFETY: the handle is open. The reset hands every buffer back, and
        // the closing callback leaves them alone.
        unsafe {
            midiInStop(handle);
            midiInReset(handle);
        }
        if let Some(buffers) = self.buffers {
            let size = size_of::<MIDIHDR>() as u32;
            // One budget of waits for all the buffers, so a driver that keeps
            // returning `MIDIERR_STILLPLAYING` delays the close no longer
            // than one buffer's retries.
            let mut waits = Waits(UNPREPARE_TRIES);
            for index in 0..BUFFERS {
                // SAFETY: the handle is open, and the reset handed this
                // buffer's header back.
                waits.retry(|| unsafe {
                    midiInUnprepareHeader(handle, buffers.header(index), size)
                });
            }
        }
        // A device that left was closed as it went, and WinMM sent
        // `MIM_CLOSE`, after which the handle is invalid and no callback
        // follows. So its buffers and ring can be freed whatever the close
        // below returns.
        // SAFETY: the handle is open and closed once, or the system closed
        // it and WinMM rejects it as invalid.
        let closed = Waits(CLOSE_TRIES).retry(|| unsafe { midiInClose(handle) }) || left;
        if !closed {
            // The driver might still write into the buffers or call back with
            // the shared state.
            self.buffers = None;
            std::mem::forget(shared);
            return None;
        }
        // A callback that began before it saw the close might still be
        // reading, but it ends without calling WinMM.
        while shared.in_callback.load(Ordering::SeqCst) != 0 {
            std::thread::yield_now();
        }
        if let Some(buffers) = self.buffers.take() {
            // SAFETY: the device is closed, so WinMM has let go of them, and
            // no callback is reading them.
            unsafe { buffers.free() };
        }
        Shared::into_producer(shared)
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        self.shut();
    }
}

/// Runs the input's thread, which hands each buffer that the callback marked
/// back to WinMM until the input stops it.
fn hand_back(handle: Handle, buffers: Buffers, shared: &Shared) {
    let size = size_of::<MIDIHDR>() as u32;
    loop {
        shared.wake.wait(INFINITE);
        if shared.closing.load(Ordering::Acquire) {
            return;
        }
        for (index, flag) in shared.returned.iter().enumerate() {
            if flag.swap(false, Ordering::AcqRel) {
                // SAFETY: the handle is open, and the driver handed this
                // prepared header back. The input stops this thread before it
                // resets the device or frees a buffer.
                unsafe { midiInAddBuffer(handle.0, buffers.header(index), size) };
            }
        }
    }
}

#[cfg(test)]
mod tests;
