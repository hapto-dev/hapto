//! Windows that a test makes for an editor's view. `HiddenWindow` is never
//! shown, and `ChildWindow` sits inside another window, as a host's does when
//! the view doesn't fill it. Each system's code is in a file of its own, such
//! as `window/win32.rs`, and a system that the test host makes no windows on
//! uses `window/none.rs`.

use std::marker::PhantomData;

use raw_window_handle::{HandleError, HasWindowHandle, WindowHandle};

#[cfg(not(windows))]
mod none;
#[cfg(windows)]
mod win32;

#[cfg(not(windows))]
use none as system;
#[cfg(windows)]
use win32 as system;

/// A window of the test's own that is never shown: a real editor opens in
/// it with no window on the screen, no focus taken and no input sent.
///
/// It isn't `Send`, so it is destroyed on the thread that made it, when it
/// drops.
///
/// # Examples
///
/// ```
#[doc = include_str!("../../../doc/editor.rs")]
/// # #[cfg(windows)]
/// # {
/// use hapto_host::vst3::{HiddenWindow, TestHost};
///
/// let host = TestHost::<Panel>::new();
/// let view = host.view().expect("the plugin has an editor");
/// let window = HiddenWindow::new().expect("a window");
/// view.attach(&window)?;
/// view.remove()?;
/// # }
/// # Ok::<(), hapto_host::vst3::Refused>(())
/// ```
#[derive(Debug)]
pub struct HiddenWindow {
    window: system::Window,
    thread: PhantomData<*const ()>,
}

impl HiddenWindow {
    /// Makes the window: a top-level one of 800 by 600 pixels that is never
    /// shown. `None` when the system refuses one, and on a system that the
    /// test host makes no windows on.
    pub fn new() -> Option<Self> {
        let window = system::Window::hidden(800, 600)?;
        Some(HiddenWindow {
            window,
            thread: PhantomData,
        })
    }

    /// Returns how many of the windows directly inside it would show, were
    /// it shown: what an editor left in it that a user would see.
    pub fn shown_children(&self) -> usize {
        self.window.shown_children()
    }
}

impl HasWindowHandle for HiddenWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        Ok(self.window.window_handle())
    }
}

/// A window inside one of the caller's, for an editor's view to attach to
/// where a host wants it: below the host's own controls, say, since an
/// editor opens at the top left of the window it is given. Its place and size
/// are in the screen's pixels, from the top left of the parent's client area.
///
/// It isn't `Send`, so it is destroyed on the thread that made it, when it
/// drops, and with its parent before that.
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
    window: system::Window,
    thread: PhantomData<*const ()>,
}

impl ChildWindow {
    /// Makes the window inside `parent`, shown, at `x`, `y`, `width` by
    /// `height`. `None` when `parent` gives no handle for this system's window
    /// type, when the system refuses a window, and on a system that the test
    /// host makes no windows on.
    pub fn new(
        parent: &impl HasWindowHandle,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Option<ChildWindow> {
        let parent = parent.window_handle().ok()?.as_raw();
        let window = system::Window::child(parent, x, y, width, height)?;
        Some(ChildWindow {
            window,
            thread: PhantomData,
        })
    }

    /// Moves it to `x`, `y` and sizes it `width` by `height`, as its host's
    /// window resizes.
    pub fn place(&self, x: i32, y: i32, width: i32, height: i32) {
        self.window.place(x, y, width, height);
    }
}

impl HasWindowHandle for ChildWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        Ok(self.window.window_handle())
    }
}
