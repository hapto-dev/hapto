//! The thread timer on macOS: a Core Foundation run loop timer on the main
//! thread's run loop, 60 times a second. It runs in the common modes, so it
//! keeps ticking while a host's modal loop runs.

use std::ffi::c_void;

use super::Tick;
use crate::boundary::guard;

type CFTypeRef = *const c_void;
type CFRunLoopRef = *mut c_void;
type CFRunLoopTimerRef = *mut c_void;
type CFStringRef = *const c_void;
type CFAllocatorRef = *const c_void;
type CFIndex = isize;
type CFOptionFlags = usize;
type Callout = unsafe extern "C" fn(CFRunLoopTimerRef, *mut c_void);

/// `CFRunLoopTimerContext`, as `CFRunLoop.h` lays it out.
#[repr(C)]
struct TimerContext {
    version: CFIndex,
    info: *mut c_void,
    retain: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
    release: Option<unsafe extern "C" fn(*const c_void)>,
    copy_description: Option<unsafe extern "C" fn(*const c_void) -> CFStringRef>,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopCommonModes: CFStringRef;
    fn CFAbsoluteTimeGetCurrent() -> f64;
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopTimerCreate(
        allocator: CFAllocatorRef,
        fire_date: f64,
        interval: f64,
        flags: CFOptionFlags,
        order: CFIndex,
        callout: Callout,
        context: *mut TimerContext,
    ) -> CFRunLoopTimerRef;
    fn CFRunLoopAddTimer(run_loop: CFRunLoopRef, timer: CFRunLoopTimerRef, mode: CFStringRef);
    fn CFRunLoopTimerInvalidate(timer: CFRunLoopTimerRef);
    fn CFRelease(object: CFTypeRef);
}

/// Seconds between ticks.
const PERIOD: f64 = 1.0 / 60.0;

/// A run loop timer whose context's info is the tick.
pub(crate) struct Timer {
    timer: CFRunLoopTimerRef,
    tick: *mut Tick,
}

/// A timer's `CFRunLoopTimer`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Handle(usize);

impl Handle {
    /// Invalidates the `CFRunLoopTimer`, so that its run loop never calls the
    /// tick again and nothing in the host points into the plugin's module. A
    /// tick that a callout is running finishes, and the tick's box leaks.
    pub(super) fn detach(self) {
        // SAFETY: the `CFRunLoopTimer` that the plugin's module made, which
        // the leaked timer's own reference keeps alive. Core Foundation's run
        // loop timers may be invalidated from any thread.
        unsafe { CFRunLoopTimerInvalidate(self.0 as CFRunLoopTimerRef) };
    }

    /// Returns whether the `CFRunLoopTimer` was invalidated, as a detached
    /// one is. Only a leaked timer's `CFRunLoopTimer` is still alive to ask.
    #[cfg(test)]
    pub(super) fn is_detached(self) -> bool {
        #[link(name = "CoreFoundation", kind = "framework")]
        unsafe extern "C" {
            fn CFRunLoopTimerIsValid(timer: CFRunLoopTimerRef) -> u8;
        }
        // SAFETY: a `CFRunLoopTimer` that the plugin's module made, which a
        // leaked timer's own reference keeps alive. Any thread may ask.
        unsafe { CFRunLoopTimerIsValid(self.0 as CFRunLoopTimerRef) == 0 }
    }
}

impl Timer {
    pub(super) fn start(tick: Tick) -> Option<Timer> {
        let tick = Box::into_raw(Box::new(tick));
        let mut context = TimerContext {
            version: 0,
            info: tick.cast(),
            retain: None,
            release: None,
            copy_description: None,
        };
        // SAFETY: a repeating timer with the default allocator. The context is
        // copied during the call, and its `info`, the tick, lives until the
        // timer's drop, after the timer is invalidated.
        let timer = unsafe {
            CFRunLoopTimerCreate(
                std::ptr::null(),
                CFAbsoluteTimeGetCurrent() + PERIOD,
                PERIOD,
                0,
                0,
                callout,
                &mut context,
            )
        };
        if timer.is_null() {
            // SAFETY: the box made above, which no timer holds.
            drop(unsafe { Box::from_raw(tick) });
            return None;
        }
        // SAFETY: the calling thread's own run loop and the timer just made.
        unsafe { CFRunLoopAddTimer(CFRunLoopGetCurrent(), timer, kCFRunLoopCommonModes) };
        Some(Timer { timer, tick })
    }

    pub(super) fn handle(&self) -> Handle {
        Handle(self.timer as usize)
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        // SAFETY: the timer that this one made, on the thread whose run loop
        // runs it, because the timer lives in main-thread state. This is
        // outside any of its callouts, because an instance released inside
        // one leaks instead. Once the timer is invalidated, no callout reaches
        // the tick, so the tick can be freed. This releases the reference that
        // `start` took.
        unsafe {
            CFRunLoopTimerInvalidate(self.timer);
            CFRelease(self.timer.cast_const());
            drop(Box::from_raw(self.tick));
        }
    }
}

impl std::fmt::Debug for Timer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Timer").finish_non_exhaustive()
    }
}

unsafe extern "C" fn callout(_timer: CFRunLoopTimerRef, info: *mut c_void) {
    let tick = info.cast_const().cast::<Tick>();
    // SAFETY: `info` is the timer's tick. The drop invalidates the timer
    // before it frees the tick, and the run loop calls this only on the
    // thread that started the timer, which is the thread that drops it, so no
    // call races the drop.
    guard((), || unsafe { (*tick)() });
}

/// Does nothing, because the timers register no class.
pub(super) fn unregister() {}

/// Runs this thread's run loop for `period`.
#[cfg(any(test, feature = "test-host"))]
pub(super) fn pump(period: std::time::Duration) {
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: CFStringRef;
        fn CFRunLoopRunInMode(mode: CFStringRef, seconds: f64, return_after_source: u8) -> i32;
    }
    // SAFETY: runs the calling thread's own run loop in its default mode, one
    // of the common modes that the timers are added to.
    unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, period.as_secs_f64(), 0) };
}
