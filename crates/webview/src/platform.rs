//! What differs between the systems that the editor runs on. Each system's
//! web engine implements [`WebEngine`], and cfg picks the one that this build
//! uses as [`Engine`].

use std::error::Error;
use std::path::PathBuf;

use wry::WebViewBuilder;

#[cfg(windows)]
mod windows;

/// The web engine of the system that this build is for.
#[cfg(windows)]
pub(crate) type Engine = windows::WebView2;

/// What the editor needs from a system's web engine.
///
/// Thread: main, for each item.
///
/// Open: `prepare` and `poll` are shaped for `WebKitGTK`, which needs GTK
/// started on the editor's thread and its events dispatched there, but no
/// engine that needs them exists yet.
pub(crate) trait WebEngine {
    /// The origin that the engine loads the page built into the plugin from.
    const ORIGIN: &'static str;

    /// Returns the folder where the engine keeps its data for the plugin
    /// whose id is `id`. Every instance of the plugin gets the same folder.
    fn data_folder(id: &str) -> PathBuf;

    /// Sets up the engine on the calling thread, for example by starting its
    /// GUI toolkit. The editor calls it at the start of every open.
    ///
    /// # Errors
    ///
    /// Returns an error when the engine can't run on this thread, and the
    /// editor's `open` returns that error.
    fn prepare() -> Result<(), Box<dyn Error>>;

    /// Adds the engine's own settings to the builder of a WebView. In a
    /// release build, they must turn off the browser's context menu and its
    /// shortcuts, such as a reload, wherever the engine has them.
    fn configure(builder: WebViewBuilder<'_>) -> WebViewBuilder<'_>;

    /// Dispatches the engine's pending events without blocking. The editor
    /// calls it on every tick. An engine whose events the host's message loop
    /// already dispatches does nothing here.
    fn poll();
}
