//! What an output's thread waits on: an event the audio thread sets once a
//! block hands the output MIDI, and a timer of the thread's own for the next
//! packet due, so the thread sleeps exactly as long as it must and wakes the
//! moment a block's MIDI arrives.
//!
//! Setting an event never blocks, so it is the one call the audio thread
//! makes. A wait's own timeout follows the system's tick, about 15.6 ms,
//! which a packet's time cannot wait on: the timer is a high-resolution one
//! where Windows has it, from Windows 10 1803 on.

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

/// An unnamed auto-reset event, which closes as it drops: what a WinMM
/// callback, an input's thread or the audio thread sets, and a thread waits
/// on. Its handle is owned, so it may be set and waited on from any thread.
pub(crate) struct EventHandle(OwnedHandle);

impl EventHandle {
    /// An event, not set; `None` if the system had none to give.
    pub(crate) fn new() -> Option<EventHandle> {
        // SAFETY: an unnamed auto-reset event, not set, with default
        // security.
        let handle = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }.ok()?;
        Some(EventHandle(owned(handle)))
    }

    /// Sets it, waking the thread that waits on it. Thread: any, the audio
    /// thread and a WinMM callback among them: it never blocks. Allocates
    /// nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub(crate) fn set(&self) {
        // SAFETY: the event this owns, alive while `self` is.
        let _ = unsafe { SetEvent(self.raw()) };
    }

    /// Waits until it is set, or `ms` milliseconds have passed.
    pub(crate) fn wait(&self, ms: u32) {
        // SAFETY: the event this owns, alive while `self` is.
        unsafe { WaitForSingleObject(self.raw(), ms) };
    }

    /// Its handle, for a call that takes one, alive while `self` is.
    pub(crate) fn raw(&self) -> HANDLE {
        HANDLE(self.0.as_raw_handle())
    }
}

/// `handle`, which the system just made and nothing else owns, owned.
fn owned(handle: HANDLE) -> OwnedHandle {
    // SAFETY: a handle the system just made, which only this takes.
    unsafe { OwnedHandle::from_raw_handle(handle.0) }
}

/// The event that wakes an output's thread: `None` if the system had none
/// to give, when the thread looks again every millisecond.
pub(crate) struct Signal(Option<EventHandle>);

impl Signal {
    pub(crate) fn new() -> Signal {
        Signal(EventHandle::new())
    }

    /// Wakes the thread that waits on it. Thread: any, the audio thread
    /// among them: it never blocks. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub(crate) fn set(&self) {
        if let Some(event) = &self.0 {
            event.set();
        }
    }
}

/// A waitable timer, the output thread's own.
pub(crate) struct Timer(Option<OwnedHandle>);

impl Timer {
    pub(crate) fn new() -> Timer {
        // SAFETY: an unnamed timer with default security, high-resolution
        // where the system has one, else as fine as the system's tick.
        let timer = unsafe {
            CreateWaitableTimerExW(
                None,
                PCWSTR::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
        }
        // SAFETY: as above, with no high resolution.
        .or_else(|_| unsafe {
            CreateWaitableTimerExW(None, PCWSTR::null(), 0, TIMER_ALL_ACCESS.0)
        });
        Timer(timer.ok().map(owned))
    }

    /// Waits until `signal` is set, or `after` has passed: for the signal
    /// alone when `after` is `None`.
    pub(crate) fn wait(&self, signal: &Signal, after: Option<Duration>) {
        let event = signal.0.as_ref().map(EventHandle::raw);
        let timer = self.0.as_ref().map(|timer| HANDLE(timer.as_raw_handle()));
        match (event, timer, after) {
            (Some(event), _, None) => {
                // SAFETY: the signal's event, alive while `signal` is.
                unsafe { WaitForSingleObject(event, INFINITE) };
            }
            (Some(event), Some(timer), Some(after)) => {
                // Negative: relative, in 100-nanosecond steps.
                let steps = i64::try_from(after.as_nanos() / 100).unwrap_or(i64::MAX);
                let due = -steps.max(1);
                // SAFETY: the timer `new` made, set once to fire after `due`,
                // with no completion routine.
                let set = unsafe { SetWaitableTimer(timer, &raw const due, 0, None, None, false) };
                if set.is_ok() {
                    // SAFETY: the signal's event and this timer, alive while
                    // they are; either one ends the wait.
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
