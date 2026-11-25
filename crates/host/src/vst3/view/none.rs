//! The view on a system with no window type for it to attach to.

use std::ffi::{CStr, c_void};

use crate::ParentWindow;

/// No platform type, because the view attaches to nothing here.
pub(super) const PLATFORM_TYPE: Option<&CStr> = None;

/// Returns `None`, because there is no window type to convert.
pub(super) fn parent_window(_: *mut c_void) -> Option<ParentWindow> {
    None
}
