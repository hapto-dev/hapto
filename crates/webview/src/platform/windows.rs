//! WebView2, the web engine on Windows.

use std::error::Error;
use std::path::PathBuf;

use wry::{WebViewBuilder, WebViewBuilderExtWindows};

use super::WebEngine;
use crate::assets::plugin_folder;

/// WebView2, which runs on the thread's own message loop. The type has no
/// values, because it only names the engine.
pub(crate) enum WebView2 {}

impl WebEngine for WebView2 {
    /// WebView2 can't register a scheme of its own, so wry serves the page's
    /// `hapto://localhost/` at `http://hapto.localhost/`.
    const ORIGIN: &'static str = "http://hapto.localhost";

    /// The folder is below the user's local application data, or below the
    /// temporary folder when `LOCALAPPDATA` is unset.
    fn data_folder(id: &str) -> PathBuf {
        let base = std::env::var_os("LOCALAPPDATA").map_or_else(std::env::temp_dir, PathBuf::from);
        plugin_folder(&base, id).join("WebView2")
    }

    /// Does nothing, because WebView2 runs on the host's own message loop.
    fn prepare() -> Result<(), Box<dyn Error>> {
        Ok(())
    }

    fn configure(builder: WebViewBuilder<'_>) -> WebViewBuilder<'_> {
        // A release build shows no browser menu and takes no browser
        // shortcut, such as a reload.
        builder
            .with_default_context_menus(cfg!(debug_assertions))
            .with_browser_accelerator_keys(cfg!(debug_assertions))
    }

    /// Does nothing, because the host's message loop dispatches WebView2's
    /// events.
    fn pump() {}
}
