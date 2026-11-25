//! The main-thread timer, for a shell whose host gives it no tick of its own.
//! VST3 gives a component no idle call without an editor. An instance has one
//! timer at most, on its main thread.
//!
//! A [`Source`] says what drives the timer. The thread timer is a
//! message-only window's timer on Windows (`windows.rs`) and a Core
//! Foundation run loop timer on macOS (`macos.rs`), and elsewhere there is
//! none (`none.rs`). The run loop timer is a handler on a VST3 host's run loop
//! (`run_loop.rs`).
//!
//! A timer calls its tick about 60 times a second, inside the panic guard,
//! until it is dropped on the thread that started it, or until its
//! [`Handle`] detaches it from any thread.

#[cfg(all(target_os = "macos", not(miri)))]
mod macos;
#[cfg(any(miri, not(any(windows, target_os = "macos"))))]
mod none;
mod run_loop;
#[cfg(all(windows, not(miri)))]
mod windows;

#[cfg(all(target_os = "macos", not(miri)))]
use macos as system;
#[cfg(any(miri, not(any(windows, target_os = "macos"))))]
use none as system;
#[cfg(all(windows, not(miri)))]
use windows as system;

use ::vst3::ComPtr;
use ::vst3::Steinberg::Linux::IRunLoop;

/// What a timer calls. A host's run loop may call it on any thread, so it is
/// `Send`.
pub(crate) type Tick = Box<dyn Fn() + Send>;

/// What drives a timer.
pub(crate) enum Source {
    /// The calling thread's own event loop.
    Thread,
    /// A VST3 host's run loop, which is the only source on Linux.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "only a Linux VST3 shell takes a host's run loop, and the exporter doesn't build there yet"
        )
    )]
    RunLoop(ComPtr<IRunLoop>),
}

/// A timer that calls its tick until it is dropped on the thread that
/// started it.
#[derive(Debug)]
pub(crate) enum Timer {
    Thread(system::Timer),
    RunLoop(run_loop::Timer),
}

/// What detaches a timer from its tick, from any thread.
#[derive(Debug, Clone)]
pub(crate) enum Handle {
    Thread(system::Handle),
    RunLoop(run_loop::Handle),
}

impl Timer {
    /// Starts calling `tick` from `source`, on the calling thread. Returns
    /// `None` if the system or the host refuses a timer.
    pub(crate) fn start(tick: Tick, source: Source) -> Option<Timer> {
        match source {
            Source::Thread => system::Timer::start(tick).map(Timer::Thread),
            Source::RunLoop(run_loop) => run_loop::Timer::start(tick, run_loop).map(Timer::RunLoop),
        }
    }

    /// Returns the handle that detaches it from any thread.
    pub(crate) fn handle(&self) -> Handle {
        match self {
            Timer::Thread(timer) => Handle::Thread(timer.handle()),
            Timer::RunLoop(timer) => Handle::RunLoop(timer.handle()),
        }
    }
}

impl Handle {
    /// Detaches the timer from its tick for good, from any thread, so that it
    /// never calls its tick again. Wherever the timer leaves the host
    /// something that points into the plugin's module, the module stays
    /// loaded. A detached timer that isn't leaked is still dropped as usual.
    pub(crate) fn detach(&self) {
        match self {
            Handle::Thread(handle) => handle.detach(),
            Handle::RunLoop(handle) => handle.detach(),
        }
    }

    /// Returns whether the timer never calls its tick again, because it was
    /// dropped or detached. For tests.
    #[cfg(test)]
    pub(crate) fn is_detached(&self) -> bool {
        match self {
            Handle::Thread(handle) => handle.is_detached(),
            Handle::RunLoop(handle) => handle.is_detached(),
        }
    }
}

/// Undoes what the first thread timer registered with the system, as the
/// host unloads the module.
pub(crate) fn unregister() {
    system::unregister();
}

/// Runs the calling thread's own event loop for `period`, as a host's would,
/// so that a thread timer started there ticks. For tests.
#[cfg(any(test, feature = "test-host"))]
pub(crate) fn pump(period: std::time::Duration) {
    system::pump(period);
}

#[cfg(test)]
mod tests;
