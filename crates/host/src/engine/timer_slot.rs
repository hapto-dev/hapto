//! The engine's own timer, which a shell starts because its host gives it no
//! tick. The timer lives in the engine's main-thread state, and its handle
//! where any thread can detach it.

use std::ptr::NonNull;
use std::sync::{MutexGuard, PoisonError};

use super::timer::{Handle, Source, Tick, Timer};
use super::{Engine, HostLink};
use crate::Plugin;
use crate::faults::Fault;

impl<P: Plugin, H: HostLink> Engine<P, H> {
    /// Starts a timer from `source` that ticks the plugin about 60 times a
    /// second on this thread, until the instance is released. If a timer
    /// already runs, it stays, and this starts none.
    ///
    /// Only an engine bound to this thread starts one, because the timer may
    /// not leave it. A call on an unbound engine, or a source that refuses a
    /// timer, counts as a fault, and the instance then never ticks.
    pub(crate) fn start_timer(&self, source: Source) {
        let Some(on_thread) = self.main.owned().and_then(|main| main.on_thread.as_ref()) else {
            self.faults.record(Fault::NoTimer);
            return;
        };
        if on_thread.timer.borrow().is_some() {
            return;
        }
        // No borrow is held while the timer starts, because a host's run loop
        // may call into the instance from inside `registerTimer`.
        let Some(timer) = Timer::start(self.ticker(), source) else {
            self.faults.record(Fault::NoTimer);
            return;
        };
        *self.handle_slot() = Some(timer.handle());
        *on_thread.timer.borrow_mut() = Some(timer);
    }

    /// Drops the running timer, on the main thread.
    pub(super) fn stop_timer(&self, timer: &mut Option<Timer>) {
        // The handle goes first, so that no other thread detaches a dropped
        // timer.
        self.handle_slot().take();
        drop(timer.take());
    }

    /// Detaches the running timer, if there is one, from any thread.
    pub(super) fn detach_timer(&self) {
        if let Some(handle) = self.handle_slot().as_ref() {
            handle.detach();
        }
    }

    /// Returns the running timer's handle.
    #[cfg(test)]
    pub(crate) fn timer_handle(&self) -> Option<Handle> {
        self.handle_slot().clone()
    }

    /// Returns the tick that a timer holds. It reaches the engine through a
    /// pointer, because a host may release the shell inside a tick.
    pub(crate) fn ticker(&self) -> Tick {
        let engine = EnginePtr(NonNull::from(self));
        Box::new(move || {
            // SAFETY: the engine never moves once its address is out, as it
            // is here. Its shell frees it only after `release` dropped the
            // timer that holds this tick, on the main thread and outside any
            // tick, and the drop waits for a tick that runs on another
            // thread. Otherwise the engine leaks. Either way, the engine is
            // live whenever a tick runs.
            unsafe { engine.get() }.timer_tick();
        })
    }

    /// Runs one `idle` for the engine's own timer. A tick on a thread other
    /// than the instance's main thread also detaches the timer, because a
    /// host that runs it there would otherwise count a fault at every tick. A
    /// timer starts only on a bound engine, so such a thread isn't the owner.
    fn timer_tick(&self) {
        self.idle();
        if !self.main.is_owner() {
            self.detach_timer();
        }
    }

    fn handle_slot(&self) -> MutexGuard<'_, Option<Handle>> {
        self.timer_handle
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// A pointer to the engine that a timer's tick holds, which may go to any
/// thread, because the engine is `Sync`.
struct EnginePtr<P: Plugin, H>(NonNull<Engine<P, H>>);

// SAFETY: a shared reference to a `Sync` engine may go to any thread, which
// the bound checks. The pointer's validity is argued where it is followed.
unsafe impl<P: Plugin, H> Send for EnginePtr<P, H> where Engine<P, H>: Sync {}

impl<P: Plugin, H> EnginePtr<P, H> {
    /// Returns the engine. It's a method, so that a closure captures the
    /// whole pointer, which is `Send`, and not its field, which isn't.
    ///
    /// # Safety
    ///
    /// The engine must be live.
    unsafe fn get(&self) -> &Engine<P, H> {
        // SAFETY: the caller's.
        unsafe { self.0.as_ref() }
    }
}
