//! The view on Windows, where it attaches to a Win32 window, an `HWND`.

use std::ffi::{CStr, c_void};
use std::num::NonZeroIsize;

use raw_window_handle::{RawWindowHandle, Win32WindowHandle};

use crate::ParentWindow;

/// The platform type that the view attaches to, as the SDK names it.
pub(super) const PLATFORM_TYPE: Option<&CStr> = Some(c"HWND");

/// Returns the host's window, an `HWND`, as an editor takes it, or `None` for
/// null.
pub(super) fn parent_window(parent: *mut c_void) -> Option<ParentWindow> {
    let hwnd = NonZeroIsize::new(parent as isize)?;
    Some(ParentWindow::new(RawWindowHandle::Win32(
        Win32WindowHandle::new(hwnd),
    )))
}
