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
pub(crate) trait WebEngine {
    /// The origin that the engine loads the page built into the plugin from.
    const ORIGIN: &'static str;

    /// Returns the folder that the engine keeps the data of the plugin `id`
    /// in. Every instance of the plugin shares it, and so shares one browser
    /// process.
    fn data_folder(id: &str) -> PathBuf;

    /// Readies the calling thread for a WebView. The editor calls it before
    /// it makes each one.
    ///
    /// # Errors
    ///
    /// When the thread can't hold a WebView. The editor's open then fails
    /// with the reason.
    fn prepare() -> Result<(), Box<dyn Error>>;

    /// Adds the engine's own settings to the builder of a WebView.
    fn configure(builder: WebViewBuilder<'_>) -> WebViewBuilder<'_>;

    /// Dispatches the engine's pending events. The editor calls it on every
    /// tick, for an engine whose events the host's event loop doesn't
    /// dispatch.
    fn pump();
}
