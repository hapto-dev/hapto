//! The window a host makes for a plugin's editor, as a safe value.

use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};

/// The host's window an [`Editor`](crate::Editor) opens inside, as a
/// toolkit takes it: through [`HasWindowHandle`].
///
/// Only an exporter makes one, from the window a host hands it or an app
/// lends it, and lends it to [`Editor::open`](crate::Editor::open) for that
/// call alone. It cannot be cloned or kept, so a handle borrowed from it
/// names the window made for that open. A host may still remove the editor
/// while `open` runs, as a message loop run inside it delivers, and destroy
/// the window before `open` returns: a toolkit's calls on it then fail, and
/// [`close`](crate::Editor::close) follows `open`.
///
/// # Examples
///
/// ```
/// use raw_window_handle::{HasWindowHandle, RawWindowHandle};
/// use hapto_host::ParentWindow;
///
/// /// What an editor's `open` reads before it makes its window inside.
/// fn platform(parent: &ParentWindow) -> &'static str {
///     match parent.window_handle().map(|handle| handle.as_raw()) {
///         Ok(RawWindowHandle::Win32(_)) => "a Win32 window",
///         Ok(RawWindowHandle::AppKit(_)) => "an AppKit view",
///         Ok(_) => "another platform's window",
///         Err(_) => "no window",
///     }
/// }
/// ```
#[derive(Debug)]
pub struct ParentWindow {
    raw: RawWindowHandle,
}

impl ParentWindow {
    /// The window `raw` names, which the host keeps valid until it removes
    /// the editor; the exporter lends it to the editor's `open` alone.
    #[cfg_attr(
        not(any(all(windows, feature = "vst3"), feature = "standalone")),
        expect(
            dead_code,
            reason = "the VST3 exporter opens an editor on Windows alone, and \
                      the standalone one in the app's window"
        )
    )]
    pub(crate) fn new(raw: RawWindowHandle) -> Self {
        ParentWindow { raw }
    }
}

impl HasWindowHandle for ParentWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: an exporter makes a `ParentWindow` only for the editor's
        // `open` and lends it to that call alone; it is not `Clone`, and
        // nothing outside this crate can make one. The VST3 exporter makes it
        // from the non-null window a host handed it to attach an editor to,
        // which the host keeps until it removes the editor; a host may do so
        // from inside `open` and destroy the window, but a Win32 window handle
        // is checked by the system at each use, so a destroyed one makes a
        // call fail and touches no memory. The standalone exporter makes it
        // from a window the app lends `StandaloneEditor::open` by reference,
        // a borrow that lasts the whole call, so that window lives while
        // `open` runs whatever the platform's handle is, a pointer into this
        // process included. `borrow_raw` asks its users to be ready for a
        // window deleted or replaced. The exporters' own tests make one from
        // a number that names no window, for stub editors that never borrow
        // it.
        Ok(unsafe { WindowHandle::borrow_raw(self.raw) })
    }
}
