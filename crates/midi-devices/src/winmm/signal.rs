//! What an output's thread waits on: an event the audio thread sets once a
//! block hands the output MIDI, and a timer of the thread's own for the next
//! packet due, so the thread sleeps exactly as long as it must and wakes the
//! moment a block's MIDI arrives.
//!
//! Setting an event never blocks, so it is the one call the audio thread
//! makes. A wait's own timeout follows the system's tick, about 15.6 ms,
//! which a packet's time cannot wait on: the timer is a high-resolution one
//! where Windows has it, from Windows 10 1803 on.

use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateEventW, CreateWaitableTimerExW, INFINITE,
    SetEvent, SetWaitableTimer, TIMER_ALL_ACCESS, WaitForMultipleObjects, WaitForSingleObject,
};
use windows::core::PCWSTR;

/// How long a thread with no event or timer to wait on sleeps at a time.
const POLL: Duration = Duration::from_millis(1);

/// An auto-reset event that wakes an output's thread: `None` if the system
/// had none to give, when the thread looks again every millisecond.
pub(crate) struct Signal(Option<HANDLE>);

// SAFETY: an event handle may be set and waited on from any thread.
unsafe impl Send for Signal {}
// SAFETY: as above; setting and waiting need no exclusive access.
unsafe impl Sync for Signal {}

impl Signal {
    pub(crate) fn new() -> Signal {
        // SAFETY: an unnamed auto-reset event, not set, with default
        // security.
        Signal(unsafe { CreateEventW(None, false, false, PCWSTR::null()) }.ok())
    }

    /// Wakes the thread that waits on it. Thread: any, the audio thread
    /// among them: it never blocks. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub(crate) fn set(&self) {
        if let Some(event) = self.0 {
            // SAFETY: the event `new` made, alive while `self` is.
            let _ = unsafe { SetEvent(event) };
        }
    }
}

impl Drop for Signal {
    fn drop(&mut self) {
        if let Some(event) = self.0 {
            // SAFETY: the event `new` made, closed once, after every holder
            // of the signal is done with it.
            let _ = unsafe { CloseHandle(event) };
        }
    }
}

/// A waitable timer, the output thread's own.
pub(crate) struct Timer(Option<HANDLE>);

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
        Timer(timer.ok())
    }

    /// Waits until `signal` is set, or `after` has passed: for the signal
    /// alone when `after` is `None`.
    pub(crate) fn wait(&self, signal: &Signal, after: Option<Duration>) {
        match (signal.0, self.0, after) {
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

impl Drop for Timer {
    fn drop(&mut self) {
        if let Some(timer) = self.0 {
            // SAFETY: the timer `new` made, closed once by its one owner.
            let _ = unsafe { CloseHandle(timer) };
        }
    }
}

#[cfg(test)]
mod tests;
