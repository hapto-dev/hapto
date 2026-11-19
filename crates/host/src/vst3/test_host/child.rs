//! A window inside a window of the caller's, where a host puts an editor's
//! view when the view does not fill its window. Windows only.

use std::num::NonZeroIsize;

use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, MoveWindow, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
    WS_VISIBLE,
};

/// A window inside one of the caller's, for an editor's view to attach to
/// where a host wants it: below the host's own controls, say, since an
/// editor opens at the top left of the window it is given. Its place and size
/// are in the screen's pixels, from the top left of the parent's client area.
/// Windows only.
///
/// It is destroyed when dropped, on the thread that made it, and with its
/// parent before that.
///
/// # Examples
///
/// ```
/// # #[cfg(windows)]
/// # {
/// use hapto_host::vst3::{ChildWindow, HiddenWindow};
///
/// let parent = HiddenWindow::new().expect("a window");
/// // The strip of 40 pixels above it is the host's own.
/// let child = ChildWindow::new(&parent, 0, 40, 400, 300).expect("a child");
/// child.place(0, 40, 500, 360);
/// assert_eq!(parent.shown_children(), 1);
/// # }
/// ```
#[derive(Debug)]
pub struct ChildWindow {
    window: HWND,
}

impl ChildWindow {
    /// Makes the window inside `parent`, shown, at `x`, `y`, `width` by
    /// `height`. `None` when `parent` gives no Win32 handle or the system
    /// refuses a window.
    pub fn new(
        parent: &impl HasWindowHandle,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Option<ChildWindow> {
        let RawWindowHandle::Win32(handle) = parent.window_handle().ok()?.as_raw() else {
            return None;
        };
        let parent = handle.hwnd.get() as HWND;
        // SAFETY: a window of the system's own static class, with a static
        // name, inside `parent`, a handle the caller lends for this call and
        // the system checks; no menu or creation data.
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
                parent,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        (!window.is_null()).then_some(ChildWindow { window })
    }

    /// Moves it to `x`, `y` and sizes it `width` by `height`, as its host's
    /// window resizes.
    pub fn place(&self, x: i32, y: i32, width: i32, height: i32) {
        // SAFETY: the window this made, on the thread that made it, as the
        // type is not `Send`. One gone with its parent makes the call fail
        // and touches no memory.
        unsafe { MoveWindow(self.window, x, y, width, height, 1) };
    }
}

impl HasWindowHandle for ChildWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let window = NonZeroIsize::new(self.window as isize).ok_or(HandleError::Unavailable)?;
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(window));
        // SAFETY: the window this made, which this destroys only when it
        // drops, so not while the handle borrows it. A handle is checked by
        // the system at each use, so one gone with its parent makes a call
        // fail and touches no memory.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

impl Drop for ChildWindow {
    fn drop(&mut self) {
        // SAFETY: the window this made, on the thread that made it. Gone
        // with its parent already, the call fails and touches no memory.
        unsafe { DestroyWindow(self.window) };
    }
}
