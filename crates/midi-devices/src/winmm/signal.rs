//! The event and timer that an output's thread waits on.
//!
//! The audio thread sets the event when a block hands the output MIDI, and
//! the thread's timer fires when the next packet is due. So the thread
//! sleeps exactly as long as it must and wakes as soon as MIDI arrives.
//!
//! Setting an event never blocks, so it is the only call that the audio
//! thread makes. A wait's own timeout follows the system tick, about
//! 15.6 ms, which is too coarse for a packet, so the timer is high-resolution
//! where Windows has it (from Windows 10 1803).

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateEventW, CreateWaitableTimerExW, INFINITE,
    SetEvent, SetWaitableTimer, TIMER_ALL_ACCESS, WaitForMultipleObjects, WaitForSingleObject,
};
use windows::core::PCWSTR;

/// How long a thread with no event or timer to wait on sleeps at a time.
const POLL: Duration = Duration::from_millis(1);

/// An unnamed auto-reset event, which closes when it drops.
///
/// A WinMM callback, an input's thread or the audio thread sets it, and a
/// thread waits on it. Its handle is owned, so any thread can use it.
pub(crate) struct EventHandle(OwnedHandle);

impl EventHandle {
    /// Creates an event, not set, or returns `None` if the system can't.
    pub(crate) fn new() -> Option<EventHandle> {
        // SAFETY: an unnamed auto-reset event, not set, with default
        // security.
        let handle = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }.ok()?;
        Some(EventHandle(owned(handle)))
    }

    /// Sets the event, waking the thread that waits on it.
    ///
    /// Thread: any, the audio thread and a WinMM callback included, because
    /// it never blocks. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub(crate) fn set(&self) {
        // SAFETY: the event that `self` owns, open while `self` lives.
        let _ = unsafe { SetEvent(self.raw()) };
    }

    /// Waits until the event is set or `ms` milliseconds have passed.
    pub(crate) fn wait(&self, ms: u32) {
        // SAFETY: the event that `self` owns, open while `self` lives.
        unsafe { WaitForSingleObject(self.raw(), ms) };
    }

    /// Returns the event's handle, valid while `self` lives.
    pub(crate) fn raw(&self) -> HANDLE {
        HANDLE(self.0.as_raw_handle())
    }
}

/// Takes ownership of `handle`, which the system just created and nothing
/// else owns.
fn owned(handle: HANDLE) -> OwnedHandle {
    // SAFETY: the system just created the handle, and only this call takes
    // it.
    unsafe { OwnedHandle::from_raw_handle(handle.0) }
}

/// The event that wakes an output's thread, or `None` if the system couldn't
/// create one, when the thread polls every millisecond.
pub(crate) struct Signal(Option<EventHandle>);

impl Signal {
    pub(crate) fn new() -> Signal {
        Signal(EventHandle::new())
    }

    /// Wakes the thread that waits on it.
    ///
    /// Thread: any, the audio thread included, because it never blocks.
    /// Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub(crate) fn set(&self) {
        if let Some(event) = &self.0 {
            event.set();
        }
    }
}

/// The output thread's own waitable timer.
pub(crate) struct Timer(Option<OwnedHandle>);

impl Timer {
    pub(crate) fn new() -> Timer {
        // SAFETY: an unnamed timer with default security, high-resolution
        // where the system has one, or else as fine as the system tick.
        let timer = unsafe {
            CreateWaitableTimerExW(
                None,
                PCWSTR::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
        }
        // SAFETY: as above, without high resolution.
        .or_else(|_| unsafe {
            CreateWaitableTimerExW(None, PCWSTR::null(), 0, TIMER_ALL_ACCESS.0)
        });
        Timer(timer.ok().map(owned))
    }

    /// Waits until `signal` is set or `after` has passed, or only for
    /// `signal` when `after` is `None`.
    pub(crate) fn wait(&self, signal: &Signal, after: Option<Duration>) {
        let event = signal.0.as_ref().map(EventHandle::raw);
        let timer = self.0.as_ref().map(|timer| HANDLE(timer.as_raw_handle()));
        match (event, timer, after) {
            (Some(event), _, None) => {
                // SAFETY: the signal's event, open while `signal` lives.
                unsafe { WaitForSingleObject(event, INFINITE) };
            }
            (Some(event), Some(timer), Some(after)) => {
                // Negative, so relative, in 100-nanosecond steps.
                let steps = i64::try_from(after.as_nanos() / 100).unwrap_or(i64::MAX);
                let due = -steps.max(1);
                // SAFETY: the timer that `new` created, set to fire once
                // after `due`, with no completion routine.
                let set = unsafe { SetWaitableTimer(timer, &raw const due, 0, None, None, false) };
                if set.is_ok() {
                    // SAFETY: the signal's event and this timer, open while
                    // `signal` and `self` live. Either one ends the wait.
                    unsafe { WaitForMultipleObjects(&[event, timer], false, INFINITE) };
                } else {
                    thread::sleep(after.min(POLL));
                }
            }
            (_, _, after) => thread::sleep(after.map_or(POLL, |after| after.min(POLL))),
        }
    }
}

#[cfg(test)]
mod tests;
