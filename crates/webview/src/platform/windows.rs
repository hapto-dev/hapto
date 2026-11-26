//! WebView2, the web engine on Windows.

use std::error::Error;

use wry::{WebViewBuilder, WebViewBuilderExtWindows};

use super::WebEngine;

/// Names WebView2 for [`Engine`](super::Engine). The enum is empty, so no
/// value of it exists.
pub(crate) enum WebView2 {}

impl WebEngine for WebView2 {
    /// WebView2 can't register a scheme of its own, so wry serves the page's
    /// `hapto://localhost/` at `http://hapto.localhost/`.
    const ORIGIN: &'static str = "http://hapto.localhost";

    const FOLDER: &'static str = "WebView2";

    /// Does nothing, because WebView2 needs no setup on the calling thread.
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
    fn poll() {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::is_page;

    #[test]
    fn the_page_built_in_is_the_page_at_webview2s_origin() {
        assert!(is_page(
            "http://hapto.localhost/index.html",
            WebView2::ORIGIN,
            None
        ));
    }

    #[test]
    fn webview2_s_folder_keeps_its_name() {
        // A new name would leave every user's WebView2 data behind.
        assert_eq!(WebView2::FOLDER, "WebView2");
    }
}
