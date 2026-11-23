//! One WinMM input, open: its callback, the thread that hands its SysEx
//! buffers back, and its close.
//!
//! WinMM calls back on a thread of its own, and documents that the callback
//! may call no multimedia function, so it only reads the message into
//! packets, pushes them into the input's ring, and marks a SysEx buffer it
//! is done with. The input's own thread hands those buffers back.

use std::fmt;
use std::panic::{self, AssertUnwindSafe};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use hapto_ump::ByteStream;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::{
    CALLBACK_FUNCTION, HMIDIIN, MIDIHDR, midiInAddBuffer, midiInClose, midiInOpen,
    midiInPrepareHeader, midiInReset, midiInStart, midiInStop, midiInUnprepareHeader,
};
use windows::Win32::Media::{
    MM_MIM_CLOSE, MM_MIM_DATA, MM_MIM_ERROR, MM_MIM_LONGDATA, MM_MIM_LONGERROR, MM_MIM_MOREDATA,
    MMSYSERR_NOERROR,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForSingleObject};
use windows::core::{PCWSTR, PSTR};

use super::{CLOSE_TRIES, UNPREPARE_TRIES, refusal, retry};
use crate::Instant;
use crate::id::{Device, Direction};
use crate::inputs::OpenError;
use crate::ring::Producer;

/// How many SysEx buffers an input keeps with the driver, and how long each
/// is: a long SysEx fills them in turn, and each comes back to be read.
const BUFFERS: usize = 16;
const BUFFER_BYTES: usize = 1024;
// A buffer's packets fit in the room the reader keeps for a burst.
const _: () = assert!(BUFFER_BYTES.div_ceil(6) < crate::inputs::BURST);

/// One SysEx buffer: its header, which WinMM reads and writes, and its
/// bytes. Aligned for the header's pointers; WinMM packs its structs.
#[repr(C, align(8))]
struct Buffer {
    header: MIDIHDR,
    bytes: [u8; BUFFER_BYTES],
}

/// A WinMM input handle, which any thread may use.
#[derive(Clone, Copy)]
struct Handle(HMIDIIN);

// SAFETY: a WinMM handle is a token the system resolves, valid on any
// thread; the input closes it only after the threads that use it are done.
unsafe impl Send for Handle {}

/// The input's buffers, which WinMM writes into while they are with it.
/// They are reached only through this pointer, never borrowed, and freed
/// only once the device is closed.
#[derive(Clone, Copy)]
struct Buffers(*mut Buffer);

// SAFETY: the buffers are WinMM's while it holds them and are reached only
// through raw pointers; the thread that hands them back stops before the
// close frees them.
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

    /// The header of buffer `index`, below [`BUFFERS`].
    fn header(self, index: usize) -> *mut MIDIHDR {
        debug_assert!(index < BUFFERS);
        // SAFETY: `index` is within the buffers, which are alive until
        // `free`.
        unsafe { &raw mut (*self.0.add(index)).header }
    }

    /// Points buffer `index`'s header at its bytes, with nothing recorded.
    fn reset(self, index: usize) {
        // SAFETY: as `header`; the buffer is not WinMM's yet, so nothing
        // else writes it.
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
    /// WinMM has let go of every one: the device is closed.
    unsafe fn free(self) {
        // SAFETY: the slice `new` made, taken back once, which the caller
        // says nothing else reaches.
        drop(unsafe { Box::from_raw(ptr::slice_from_raw_parts_mut(self.0, BUFFERS)) });
    }
}

/// An auto-reset event: the callback sets it, which WinMM allows, and the
/// input's thread waits on it.
struct Wake(HANDLE);

// SAFETY: an event handle may be set and waited on from any thread.
unsafe impl Send for Wake {}
// SAFETY: as above; setting and waiting need no exclusive access.
unsafe impl Sync for Wake {}

impl Drop for Wake {
    fn drop(&mut self) {
        // SAFETY: the event `CreateEventW` made, closed once, after every
        // thread that sets or waits on it is done.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// What the callback reads into.
struct Reading {
    bytes: ByteStream,
    producer: Producer,
}

/// What the input shares with its callback and its thread. It outlives
/// every callback: it is let go only after the device closes, and leaked if
/// it does not.
struct Shared {
    /// Set as the close begins: from then on the callback reads nothing and
    /// hands no buffer back.
    closing: AtomicBool,
    /// Set when the device closed without the input closing it: it left.
    left: AtomicBool,
    /// When the device started, on the clock's nanoseconds: WinMM stamps a
    /// message with the milliseconds since.
    started: AtomicU64,
    /// Locked by the callback alone while the device runs.
    reading: Mutex<Reading>,
    /// Which buffers came back, for the input's thread to hand back.
    returned: [AtomicBool; BUFFERS],
    wake: Wake,
    /// Tells the input's thread to end.
    stop: AtomicBool,
    /// How many callbacks are running, which the close waits out before it
    /// frees what they read.
    in_callback: AtomicU32,
}

impl Shared {
    fn new(producer: Producer, wake: Wake) -> Shared {
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
            stop: AtomicBool::new(false),
            in_callback: AtomicU32::new(0),
        }
    }

    /// One message from WinMM: `first` and `second` are its two
    /// parameters.
    fn message(&self, message: u32, first: usize, second: usize) {
        // Sequentially consistent with the close's store and its wait on
        // `in_callback`: a callback either sees the close, or is counted
        // before the close waits.
        if self.closing.load(Ordering::SeqCst) {
            return;
        }
        let at = self.stamp(second);
        match message {
            MM_MIM_DATA | MM_MIM_MOREDATA => {
                // A whole message, running status filled in: its status in
                // the low byte, then its data.
                let packed = (first as u32).to_le_bytes();
                let length = match packed[0] {
                    0xF6 | 0xF8..=0xFF => 1,
                    0xC0..=0xDF | 0xF1 | 0xF3 => 2,
                    _ => 3,
                };
                self.read(at, packed.get(..length).unwrap_or(&[]));
            }
            MM_MIM_LONGDATA | MM_MIM_LONGERROR => {
                let header = first as *const MIDIHDR;
                // SAFETY: WinMM hands back one of the input's headers, which
                // live until the close frees them after the device closes.
                // Read by value: the struct is packed.
                let header = unsafe { ptr::read_unaligned(header) };
                if message == MM_MIM_LONGDATA {
                    let recorded = (header.dwBytesRecorded as usize).min(BUFFER_BYTES);
                    // SAFETY: the driver wrote `recorded` bytes into the
                    // buffer's own, and is done with it until it is handed
                    // back.
                    let bytes = unsafe { std::slice::from_raw_parts(header.lpData.0, recorded) };
                    self.read(at, bytes);
                } else {
                    // A SysEx the driver could not read whole ends where it
                    // broke off, so the reader is not left holding it.
                    self.cut(at);
                }
                self.hand_back(header.dwUser);
            }
            MM_MIM_ERROR => self.cut(at),
            MM_MIM_CLOSE => self.left.store(true, Ordering::Release),
            _ => {}
        }
    }

    /// When a message WinMM stamped `millis` came.
    fn stamp(&self, millis: usize) -> Instant {
        let started = Instant::from_nanos(self.started.load(Ordering::Acquire));
        started.after_millis(u32::try_from(millis).unwrap_or(u32::MAX))
    }

    fn read(&self, at: Instant, bytes: &[u8]) {
        let mut reading = self.lock();
        let Reading {
            bytes: stream,
            producer,
        } = &mut *reading;
        stream.read(bytes, |words| producer.push(at, words));
    }

    /// Ends a SysEx under way at `at`, as the stream cuts it.
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
            // SAFETY: the shared event, alive with this. WinMM lets a
            // callback set an event.
            let _ = unsafe { SetEvent(self.wake.0) };
        }
    }

    /// The ring's producer, once nothing else holds the shared state.
    fn into_producer(this: Arc<Shared>) -> Option<Producer> {
        let shared = Arc::try_unwrap(this).ok()?;
        let Reading {
            bytes: mut stream,
            mut producer,
        } = shared
            .reading
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        // A SysEx the close cut short ends there, so the reader does not
        // hold it, and the real-time messages behind it, for good.
        let now = Instant::now();
        stream.cut(|words| producer.push(now, words));
        Some(producer)
    }
}

/// WinMM's callback. It reads, pushes and marks, never calling WinMM, and
/// a panic in it is caught: none crosses into the system.
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

/// An input, reading into its ring while open. It closes as it drops.
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
    /// Opens `device` to read into `producer`'s ring. On a refusal the
    /// producer comes back, unless the system would not close what it half
    /// opened.
    pub(crate) fn open(
        device: &Device,
        producer: Producer,
    ) -> Result<Input, (OpenError, Option<Producer>)> {
        let Some(index) = super::index_of(Direction::Input, device.id()) else {
            return Err((OpenError::Gone, Some(producer)));
        };
        // SAFETY: an unnamed auto-reset event, not set, with default
        // security.
        let Ok(event) = (unsafe { CreateEventW(None, false, false, PCWSTR::null()) }) else {
            return Err((OpenError::NoMemory, Some(producer)));
        };
        let shared = Arc::new(Shared::new(producer, Wake(event)));
        let mut handle = HMIDIIN::default();
        let callback = callback as unsafe extern "system" fn(HMIDIIN, u32, usize, usize, usize);
        // SAFETY: the handle is written by the call. The callback is this
        // module's, and its instance the shared state, which outlives every
        // callback as `Shared` says.
        let opened = unsafe {
            midiInOpen(
                &raw mut handle,
                index,
                Some(callback as usize),
                Some(Arc::as_ptr(&shared) as usize),
                CALLBACK_FUNCTION,
            )
        };
        if opened != MMSYSERR_NOERROR {
            return Err((refusal(opened), Shared::into_producer(shared)));
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
            // SAFETY: the open handle and one of its buffers' headers, not
            // yet WinMM's.
            let prepared =
                unsafe { midiInPrepareHeader(self.handle.0, buffers.header(index), size) };
            // SAFETY: the same header, prepared.
            let added = unsafe { midiInAddBuffer(self.handle.0, buffers.header(index), size) };
            if prepared != MMSYSERR_NOERROR || added != MMSYSERR_NOERROR {
                return Err(refusal(if prepared == MMSYSERR_NOERROR {
                    added
                } else {
                    prepared
                }));
            }
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
        // SAFETY: the open handle, its buffers with it.
        let started = unsafe { midiInStart(self.handle.0) };
        if started != MMSYSERR_NOERROR {
            return Err(refusal(started));
        }
        Ok(())
    }

    /// Whether the device left while open.
    pub(crate) fn has_left(&self) -> bool {
        self.shared
            .as_ref()
            .is_some_and(|shared| shared.left.load(Ordering::Acquire))
    }

    /// Closes the device and gives its ring's producer back, unless the
    /// system would not close it.
    pub(crate) fn close(mut self) -> Option<Producer> {
        self.shut()
    }

    /// Stops the thread, then stops, resets and closes the device, taking
    /// its buffers back. If the device does not close, the buffers and the
    /// shared state are leaked rather than freed under the driver.
    fn shut(&mut self) -> Option<Producer> {
        let shared = self.shared.take()?;
        shared.closing.store(true, Ordering::SeqCst);
        let left = shared.left.load(Ordering::Acquire);
        shared.stop.store(true, Ordering::Release);
        // SAFETY: the shared event, alive with it.
        let _ = unsafe { SetEvent(shared.wake.0) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let handle = self.handle.0;
        // SAFETY: the open handle. The reset hands every buffer back, which
        // the callback, closing, leaves alone.
        unsafe {
            midiInStop(handle);
            midiInReset(handle);
        }
        if let Some(buffers) = self.buffers {
            let size = size_of::<MIDIHDR>() as u32;
            // One budget of waits for every buffer, so a driver that keeps
            // answering that one is still playing holds the close no longer
            // than a buffer's worth.
            let mut waits = UNPREPARE_TRIES;
            for index in 0..BUFFERS {
                // SAFETY: the open handle and one of its buffers' headers,
                // back from the reset.
                retry(&mut waits, || unsafe {
                    midiInUnprepareHeader(handle, buffers.header(index), size)
                });
            }
        }
        // A device that left closed as it went, and said so: WinMM's
        // handle is no longer valid after `MIM_CLOSE`, and no callback
        // follows, so its buffers and ring are free whatever the close
        // below answers.
        // SAFETY: the open handle, closed once; or one the system closed,
        // which WinMM refuses as invalid.
        let closed = retry(&mut CLOSE_TRIES.clone(), || unsafe { midiInClose(handle) }) || left;
        if !closed {
            // The driver may still write into the buffers or call back with
            // the shared state.
            self.buffers = None;
            std::mem::forget(shared);
            return None;
        }
        // A callback that began before the close was seen may still be
        // reading; it ends without calling WinMM.
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

/// The input's thread: it hands each buffer the callback marked back to
/// WinMM, until the input stops it.
fn hand_back(handle: Handle, buffers: Buffers, shared: &Shared) {
    let size = size_of::<MIDIHDR>() as u32;
    loop {
        // SAFETY: the shared event, alive while this thread holds the
        // shared state.
        unsafe { WaitForSingleObject(shared.wake.0, INFINITE) };
        if shared.stop.load(Ordering::Acquire) {
            return;
        }
        for (index, flag) in shared.returned.iter().enumerate() {
            if flag.swap(false, Ordering::AcqRel) {
                // SAFETY: the open handle and one of its prepared headers,
                // back from the driver. The input stops this thread before
                // it resets the device or frees a buffer.
                unsafe { midiInAddBuffer(handle.0, buffers.header(index), size) };
            }
        }
    }
}

#[cfg(test)]
mod tests;
