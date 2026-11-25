//! The thread timer on Windows: a message-only window, made on the main
//! thread, that ticks through that thread's message loop. It asks for 15 ms,
//! which the system clock's 15.6 ms tick rounds to, so it ticks about 64
//! times a second.

use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW, HWND_MESSAGE,
    KillTimer, RegisterClassW, SetTimer, SetWindowLongPtrW, UnregisterClassW, WM_TIMER, WNDCLASSW,
};
use windows_sys::core::PCWSTR;

use super::Tick;
use crate::boundary::guard;
use crate::engine::module;

/// The window class's name, a static UTF-16 string with its null. The class
/// belongs to the plugin's module.
const CLASS: PCWSTR = windows_sys::w!("HaptoTimer");
static REGISTERED: AtomicBool = AtomicBool::new(false);
const ID: usize = 1;
const PERIOD_MS: u32 = 15;

fn register() -> bool {
    if REGISTERED.load(Ordering::Acquire) {
        return true;
    }
    let class = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(window_procedure),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: module::handle(),
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: CLASS,
    };
    // SAFETY: a filled class description whose strings are static.
    let registered = unsafe { RegisterClassW(&class) } != 0
        // SAFETY: reads this thread's last error.
        || unsafe { GetLastError() } == ERROR_CLASS_ALREADY_EXISTS;
    REGISTERED.store(registered, Ordering::Release);
    registered
}

/// Unregisters the window class.
pub(super) fn unregister() {
    if REGISTERED.swap(false, Ordering::AcqRel) {
        // SAFETY: the class that the plugin's module registered. A dropped
        // timer has destroyed its window, but a detached one's is never
        // destroyed.
        // While one is left, the call fails and unregisters nothing, which is
        // harmless, because that window's module is pinned, so its class and
        // procedure stay mapped.
        unsafe { UnregisterClassW(CLASS, module::handle()) };
    }
}

/// A message-only window with a timer, whose user data is the tick.
pub(crate) struct Timer {
    window: HWND,
    tick: *mut Tick,
}

/// A timer's window.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Handle(usize);

impl Handle {
    /// Clears the window's tick. The window procedure kills the timer at its
    /// next `WM_TIMER`, when it finds no tick. The window is never destroyed,
    /// so this pins the plugin's module.
    pub(super) fn detach(self) {
        module::keep_loaded();
        // SAFETY: a window that the plugin's module made and never destroys.
        // Any thread of the process may set its user data, and the procedure
        // reads zero as no tick.
        unsafe { SetWindowLongPtrW(self.0 as HWND, GWLP_USERDATA, 0) };
    }

    /// Returns whether the window procedure finds no tick, because the timer
    /// was dropped, which destroyed its window, or detached.
    #[cfg(test)]
    pub(super) fn is_detached(self) -> bool {
        // SAFETY: reads the user data of a window that the plugin's module
        // made, from any thread. A destroyed window's read returns 0, as a
        // detached one's does.
        unsafe { GetWindowLongPtrW(self.0 as HWND, GWLP_USERDATA) == 0 }
    }

    /// Returns whether the timer's window takes no more of its messages. It
    /// waits four periods and checks that no `WM_TIMER` is due. On the
    /// window's thread.
    #[cfg(test)]
    pub(super) fn is_stopped(self) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, PM_REMOVE, PeekMessageW};

        std::thread::sleep(std::time::Duration::from_millis(u64::from(4 * PERIOD_MS)));
        // SAFETY: all zeros is a valid message, which `PeekMessageW`
        // overwrites.
        let mut message: MSG = unsafe { std::mem::zeroed() };
        // SAFETY: takes a timer message for a window that this thread made, if
        // one is due.
        unsafe { PeekMessageW(&mut message, self.0 as HWND, WM_TIMER, WM_TIMER, PM_REMOVE) == 0 }
    }
}

impl Timer {
    pub(super) fn start(tick: Tick) -> Option<Timer> {
        if !register() {
            return None;
        }
        // SAFETY: a message-only window of the class just registered, with no
        // name, parent or menu.
        let window = unsafe {
            CreateWindowExW(
                0,
                CLASS,
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                module::handle(),
                std::ptr::null(),
            )
        };
        if window.is_null() {
            return None;
        }
        let timer = Timer {
            window,
            tick: Box::into_raw(Box::new(tick)),
        };
        // SAFETY: the window just made. Its user data is the tick, which lives
        // until the window is destroyed in `drop`.
        unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, timer.tick as isize) };
        // SAFETY: a timer on the window just made, which posts `WM_TIMER`.
        if unsafe { SetTimer(window, ID, PERIOD_MS, None) } == 0 {
            return None;
        }
        Some(timer)
    }

    pub(super) fn handle(&self) -> Handle {
        Handle(self.window as usize)
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        // SAFETY: the window this timer made, on the thread that made it,
        // because the timer lives in main-thread state. After these calls no
        // message reaches the window, so the tick can be freed.
        unsafe {
            KillTimer(self.window, ID);
            SetWindowLongPtrW(self.window, GWLP_USERDATA, 0);
            DestroyWindow(self.window);
            drop(Box::from_raw(self.tick));
        }
    }
}

impl std::fmt::Debug for Timer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Timer").finish_non_exhaustive()
    }
}

unsafe extern "system" fn window_procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_TIMER && wparam == ID {
        // SAFETY: the window's user data is its timer's tick, or zero once the
        // timer is being dropped or was detached.
        let tick = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *const Tick;
        if tick.is_null() {
            // SAFETY: the timer set on this window, on this thread, which made
            // it. A detached timer stops here.
            unsafe { KillTimer(window, ID) };
        } else {
            // SAFETY: a non-zero user data is the timer's tick, which lives
            // until the timer's drop clears it. Windows runs a window's
            // procedure only on the thread that made the window, which is the
            // thread that drops the timer, so no call races the drop.
            guard((), || unsafe { (*tick)() });
        }
        return 0;
    }
    // SAFETY: every other message goes to the default procedure.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

/// Runs this thread's message loop for `period`.
#[cfg(any(test, feature = "test-host"))]
pub(super) fn pump(period: std::time::Duration) {
    use std::time::{Duration, Instant};

    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW,
    };

    let end = Instant::now() + period;
    while Instant::now() < end {
        // SAFETY: all zeros is a valid message, which `PeekMessageW`
        // overwrites.
        let mut message: MSG = unsafe { std::mem::zeroed() };
        // SAFETY: reads and dispatches this thread's own messages.
        while unsafe { PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            // SAFETY: a message just taken from the queue.
            unsafe { DispatchMessageW(&message) };
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
