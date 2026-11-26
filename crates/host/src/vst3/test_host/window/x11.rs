//! The test host's windows on Linux, which are X11 windows made through
//! x11rb. Each window has a connection to the X server of its own, so the
//! server destroys the window, and every window inside it, when the
//! connection closes.
//!
//! The server checks a window's id at each request, so a request on a window
//! that is gone, destroyed with its parent, fails and touches no memory here.

use std::ffi::c_ulong;
use std::num::NonZeroU32;

use raw_window_handle::{RawWindowHandle, WindowHandle, XlibWindowHandle};
use x11rb::connection::Connection as _;
use x11rb::cookie::VoidCookie;
use x11rb::protocol::xproto::{
    ConfigureWindowAux, ConnectionExt as _, CreateWindowAux, MapState, WindowClass,
};
use x11rb::rust_connection::RustConnection;

/// An X11 window that this type creates. Dropping it closes the connection,
/// so the server destroys the window.
#[derive(Debug)]
pub(super) struct Window {
    connection: RustConnection,
    id: NonZeroU32,
}

impl Window {
    /// Makes a top-level window of `width` by `height` that is never shown.
    /// `None` when there is no X server to connect to.
    pub(super) fn hidden(width: i32, height: i32) -> Option<Window> {
        let (connection, screen) = x11rb::connect(None).ok()?;
        let root = connection.setup().roots.get(screen)?.root;
        Window::create(connection, root, (0, 0, width, height))
    }

    /// Makes a shown window inside `parent`, at `x`, `y`, `width` by
    /// `height`. `None` for a parent that isn't an X11 window.
    pub(super) fn child(
        parent: RawWindowHandle,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Option<Window> {
        let RawWindowHandle::Xlib(parent) = parent else {
            return None;
        };
        let parent = u32::try_from(parent.window).ok()?;
        let (connection, _) = x11rb::connect(None).ok()?;
        let window = Window::create(connection, parent, (x, y, width, height))?;
        window
            .connection
            .map_window(window.id.get())
            .ok()?
            .check()
            .ok()?;
        Some(window)
    }

    /// Makes a window inside `parent` at `x`, `y`, `width` by `height`, not
    /// yet mapped. A request that fails after the window exists leaves it to
    /// the connection's close, which destroys it.
    fn create(
        connection: RustConnection,
        parent: u32,
        (x, y, width, height): (i32, i32, i32, i32),
    ) -> Option<Window> {
        let id = NonZeroU32::new(connection.generate_id().ok()?)?;
        connection
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                id.get(),
                parent,
                i16::try_from(x).ok()?,
                i16::try_from(y).ok()?,
                u16::try_from(width).ok()?,
                u16::try_from(height).ok()?,
                0,
                WindowClass::INPUT_OUTPUT,
                x11rb::COPY_FROM_PARENT,
                &CreateWindowAux::new(),
            )
            .ok()?
            .check()
            .ok()?;
        Some(Window { connection, id })
    }

    pub(super) fn place(&self, x: i32, y: i32, width: i32, height: i32) {
        let place = ConfigureWindowAux::new()
            .x(x)
            .y(y)
            .width(u32::try_from(width).unwrap_or(0))
            .height(u32::try_from(height).unwrap_or(0));
        // A window that the host destroyed can't be placed, as on Win32.
        let _ = self
            .connection
            .configure_window(self.id.get(), &place)
            .map(VoidCookie::ignore_error);
        let _ = self.connection.flush();
    }

    pub(super) fn shown_children(&self) -> usize {
        let Some(tree) = self
            .connection
            .query_tree(self.id.get())
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        else {
            return 0;
        };
        // A hidden window is never mapped, so the server reports a mapped
        // child of it as unviewable, which counts as shown here.
        tree.children
            .iter()
            .filter_map(|&child| self.connection.get_window_attributes(child).ok())
            .filter_map(|cookie| cookie.reply().ok())
            .filter(|attributes| attributes.map_state != MapState::UNMAPPED)
            .count()
    }

    pub(super) fn window_handle(&self) -> WindowHandle<'_> {
        let raw = RawWindowHandle::Xlib(XlibWindowHandle::new(c_ulong::from(self.id.get())));
        // SAFETY: an X11 id is not memory, and a request on a destroyed
        // window gets an error from the server, so the handle is safe for as
        // long as it borrows `self`.
        unsafe { WindowHandle::borrow_raw(raw) }
    }
}
