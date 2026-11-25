//! The run loop timer: a handler that a VST3 host's run loop (`IRunLoop`)
//! calls, which a Linux host offers. The SDK defines the interface for Linux,
//! but the bindings define it on every system, so the timer builds and is
//! tested on each.
//!
//! A host should call the handler on its UI thread, the instance's main
//! thread. A faulty host may call it on another thread, so the tick is `Send`
//! and runs under a lock, which the timer's drop takes to wait for it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use ::vst3::Steinberg::Linux::{IRunLoop, IRunLoopTrait, ITimerHandler, ITimerHandlerTrait};
use ::vst3::Steinberg::kResultOk;
use ::vst3::{Class, ComPtr, ComWrapper};

use super::Tick;
use crate::boundary::guard;
use crate::engine::module;

/// The milliseconds between ticks, about 60 a second.
pub(super) const PERIOD_MS: u64 = 16;

/// A handler registered with a host's run loop.
pub(crate) struct Timer {
    run_loop: ComPtr<IRunLoop>,
    handler: ComWrapper<Handler>,
}

/// A timer's handler.
#[derive(Clone)]
pub(crate) struct Handle(ComWrapper<Handler>);

/// The COM object that the run loop calls.
struct Handler {
    /// The tick, until it is taken. A tick holds the lock while it runs.
    tick: Mutex<Option<Tick>>,
    /// Whether the timer was detached, after which no tick runs.
    detached: AtomicBool,
}

impl Class for Handler {
    type Interfaces = (ITimerHandler,);
}

impl ITimerHandlerTrait for Handler {
    unsafe fn onTimer(&self) {
        if self.detached.load(Ordering::Acquire) {
            return;
        }
        // A tick that is already running, nested in the host's loop or on
        // another thread, holds the lock, and so does a timer being dropped.
        // This tick then does nothing.
        let Ok(tick) = self.tick.try_lock() else {
            return;
        };
        if let Some(tick) = tick.as_ref() {
            guard((), tick);
        }
    }
}

impl Handler {
    /// Takes the tick, after waiting for one that runs on another thread, so
    /// that none runs once this returns.
    fn take_tick(&self) {
        let mut tick = self.tick.lock().unwrap_or_else(PoisonError::into_inner);
        drop(tick.take());
    }
}

impl Timer {
    pub(super) fn start(tick: Tick, run_loop: ComPtr<IRunLoop>) -> Option<Timer> {
        let handler = ComWrapper::new(Handler {
            tick: Mutex::new(Some(tick)),
            detached: AtomicBool::new(false),
        });
        let pointer = handler.as_com_ref::<ITimerHandler>()?.as_ptr();
        // SAFETY: the host's live run loop, on the thread that it runs on, and
        // a live handler, which the timer keeps until its drop unregisters it.
        if unsafe { run_loop.registerTimer(pointer, PERIOD_MS) } != kResultOk {
            // A faulty host may keep the handler that it refused and call it
            // later, when the instance that the tick reaches may be freed.
            handler.take_tick();
            return None;
        }
        Some(Timer { run_loop, handler })
    }

    pub(super) fn handle(&self) -> Handle {
        Handle(self.handler.clone())
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        if let Some(handler) = self.handler.as_com_ref::<ITimerHandler>() {
            // SAFETY: the run loop that the handler is registered with, on the
            // thread that started the timer, because the timer lives in
            // main-thread state.
            unsafe { self.run_loop.unregisterTimer(handler.as_ptr()) };
        }
        self.handler.take_tick();
    }
}

impl std::fmt::Debug for Timer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Timer").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handle").finish_non_exhaustive()
    }
}

impl Handle {
    /// Marks the handler as detached. The host's run loop may still hold the
    /// handler, whose code is in the plugin's module, so this asks for the
    /// module to stay loaded.
    pub(super) fn detach(&self) {
        module::keep_loaded();
        self.0.detached.store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(super) fn is_detached(&self) -> bool {
        let taken = self.0.tick.lock().map_or(true, |tick| tick.is_none());
        self.0.detached.load(Ordering::Acquire) || taken
    }
}
