//! The test host's windows on a system that it makes no windows on, where
//! no window can exist.

use raw_window_handle::{RawWindowHandle, WindowHandle};

/// A window, which can't exist here.
#[derive(Debug)]
pub(super) enum Window {}

impl Window {
    pub(super) fn hidden(_: i32, _: i32) -> Option<Window> {
        None
    }

    pub(super) fn child(_: RawWindowHandle, _: i32, _: i32, _: i32, _: i32) -> Option<Window> {
        None
    }

    pub(super) fn place(&self, _: i32, _: i32, _: i32, _: i32) {
        match *self {}
    }

    pub(super) fn shown_children(&self) -> usize {
        match *self {}
    }

    pub(super) fn window_handle(&self) -> WindowHandle<'_> {
        match *self {}
    }
}
