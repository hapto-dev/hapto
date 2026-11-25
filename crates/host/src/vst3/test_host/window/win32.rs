//! The test host's windows on Windows, which are Win32 windows of the
//! system's `STATIC` class.
//!
//! The system checks a window handle at each use, so a call on a window that
//! is gone, destroyed with its parent, fails and touches no memory.

use std::num::NonZeroIsize;

use raw_window_handle::{RawWindowHandle, Win32WindowHandle, WindowHandle};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GW_CHILD, GW_HWNDNEXT, GWL_STYLE, GetWindow, GetWindowLongPtrW,
    MoveWindow, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

/// A Win32 window that this type creates, and destroys when it drops. Its
/// wrapper isn't `Send`, so every call is on the thread that created it.
#[derive(Debug)]
pub(super) struct Window(NonZeroIsize);

impl Window {
    /// Makes a top-level window of `width` by `height` that is never shown.
    pub(super) fn hidden(width: i32, height: i32) -> Option<Window> {
        // SAFETY: a window of the system's own static class, with a static
        // name and no parent, menu or creation data. It is never shown,
        // because no style makes it visible.
        let window = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                windows_sys::w!("STATIC"),
                windows_sys::w!("hapto test window"),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                0,
                0,
                width,
                height,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        NonZeroIsize::new(window as isize).map(Window)
    }

    /// Makes a shown window inside `parent`, at `x`, `y`, `width` by
    /// `height`. `None` for a parent that isn't a Win32 window.
    pub(super) fn child(
        parent: RawWindowHandle,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Option<Window> {
        let RawWindowHandle::Win32(parent) = parent else {
            return None;
        };
        // SAFETY: a window of the system's own static class, with a static
        // name, inside `parent`, a handle that the caller lends for this call.
        // No menu or creation data.
        let window = unsafe {
            CreateWindowExW(
                0,
                windows_sys::w!("STATIC"),
                windows_sys::w!(""),
                WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                x,
                y,
                width,
                height,
                parent.hwnd.get() as HWND,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        NonZeroIsize::new(window as isize).map(Window)
    }

    fn hwnd(&self) -> HWND {
        self.0.get() as HWND
    }

    pub(super) fn place(&self, x: i32, y: i32, width: i32, height: i32) {
        // SAFETY: the window that this created.
        unsafe { MoveWindow(self.hwnd(), x, y, width, height, 1) };
    }

    pub(super) fn shown_children(&self) -> usize {
        let mut shown = 0;
        // SAFETY: the window that this created. The call only reads.
        let mut child = unsafe { GetWindow(self.hwnd(), GW_CHILD) };
        while !child.is_null() {
            // SAFETY: a window that the system named as this one's child just
            // now, which no other code here destroys. The calls only read.
            let style = unsafe { GetWindowLongPtrW(child, GWL_STYLE) } as u32;
            if style & WS_VISIBLE != 0 {
                shown += 1;
            }
            // SAFETY: as above.
            child = unsafe { GetWindow(child, GW_HWNDNEXT) };
        }
        shown
    }

    pub(super) fn window_handle(&self) -> WindowHandle<'_> {
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(self.0));
        // SAFETY: the window that this created, which this destroys only when
        // it drops, so not while the handle borrows it.
        unsafe { WindowHandle::borrow_raw(raw) }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // SAFETY: the window that this created.
        unsafe { DestroyWindow(self.hwnd()) };
    }
}
