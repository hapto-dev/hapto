//! A VST3 host's run loop for tests: a COM object that timers register with,
//! as a Linux host offers one. A test calls [`TestRunLoop::fire`] where a
//! host's loop would call each timer when its time comes, so no test waits
//! on a clock.

use std::sync::{Mutex, MutexGuard, PoisonError};

use ::vst3::Steinberg::Linux::{
    FileDescriptor, IEventHandler, IRunLoop, IRunLoopTrait, ITimerHandler, ITimerHandlerTrait,
    TimerInterval,
};
use ::vst3::Steinberg::{kInvalidArgument, kNotImplemented, kResultFalse, kResultOk, tresult};
use ::vst3::{Class, ComPtr, ComRef, ComWrapper};

use crate::engine::timer::Source;

/// A run loop whose timers a test fires.
pub(crate) struct TestRunLoop {
    timers: Mutex<Vec<Registered>>,
    /// Whether `registerTimer` refuses each timer, but keeps it and calls it
    /// all the same, as a faulty host might.
    refuses: bool,
}

/// A timer registered with the run loop.
struct Registered {
    handler: ComPtr<ITimerHandler>,
    milliseconds: TimerInterval,
}

impl Class for TestRunLoop {
    type Interfaces = (IRunLoop,);
}

impl TestRunLoop {
    /// Makes a run loop with no timers.
    pub(crate) fn new() -> ComWrapper<TestRunLoop> {
        Self::make(false)
    }

    /// Makes a run loop that refuses each timer, but keeps it and calls it
    /// all the same, as a faulty host might.
    pub(crate) fn refusing() -> ComWrapper<TestRunLoop> {
        Self::make(true)
    }

    fn make(refuses: bool) -> ComWrapper<TestRunLoop> {
        ComWrapper::new(TestRunLoop {
            timers: Mutex::new(Vec::new()),
            refuses,
        })
    }

    /// Returns the run loop as the source that a host hands a plugin.
    pub(crate) fn source(run_loop: &ComWrapper<TestRunLoop>) -> Source {
        Source::RunLoop(run_loop.to_com_ptr().expect("an IRunLoop"))
    }

    /// Calls each registered timer once, on the calling thread. A timer that
    /// a tick unregisters is still called in this round.
    pub(crate) fn fire(&self) {
        let handlers: Vec<_> = self
            .timers()
            .iter()
            .map(|timer| timer.handler.clone())
            .collect();
        for handler in handlers {
            // SAFETY: a live handler, which the clone's reference keeps alive.
            unsafe { handler.onTimer() };
        }
    }

    /// Returns the interval of each registered timer, in milliseconds.
    pub(crate) fn intervals(&self) -> Vec<TimerInterval> {
        self.timers()
            .iter()
            .map(|timer| timer.milliseconds)
            .collect()
    }

    fn timers(&self) -> MutexGuard<'_, Vec<Registered>> {
        self.timers.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl IRunLoopTrait for TestRunLoop {
    unsafe fn registerEventHandler(
        &self,
        _handler: *mut IEventHandler,
        _fd: FileDescriptor,
    ) -> tresult {
        kNotImplemented
    }

    unsafe fn unregisterEventHandler(&self, _handler: *mut IEventHandler) -> tresult {
        kNotImplemented
    }

    unsafe fn registerTimer(
        &self,
        handler: *mut ITimerHandler,
        milliseconds: TimerInterval,
    ) -> tresult {
        // SAFETY: the plugin passes a live handler, or null.
        let Some(handler) = (unsafe { ComRef::from_raw(handler) }) else {
            return kInvalidArgument;
        };
        self.timers().push(Registered {
            handler: handler.to_com_ptr(),
            milliseconds,
        });
        if self.refuses {
            kResultFalse
        } else {
            kResultOk
        }
    }

    unsafe fn unregisterTimer(&self, handler: *mut ITimerHandler) -> tresult {
        let mut timers = self.timers();
        let before = timers.len();
        timers.retain(|timer| timer.handler.as_ptr() != handler);
        if timers.len() < before {
            kResultOk
        } else {
            kInvalidArgument
        }
    }
}
