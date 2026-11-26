//! WebView2, the web engine on Windows, in the plugin's own process.

use std::path::PathBuf;

use hapto_host::{ParentWindow, Size};
use wry::dpi::{PhysicalPosition, PhysicalSize};
use wry::{Rect, WebContext, WebView, WebViewBuilderExtWindows};

use super::{Load, WebEngine};
use crate::wry_page;

/// The page in a WebView2 window, a child of the host's window.
pub(crate) struct WebView2(WebView);

impl WebView2 {
    /// The origin that WebView2 loads the page built into the plugin from.
    /// WebView2 can't register a scheme of its own, so wry serves the page's
    /// `hapto://localhost/` at `http://hapto.localhost/`.
    const ORIGIN: &'static str = "http://hapto.localhost";
}

impl WebEngine for WebView2 {
    const FOLDER: &'static str = "WebView2";

    fn open(load: Load, parent: &ParentWindow, size: Size, scale: f64) -> Result<WebView2, String> {
        let mut context = WebContext::new(Some(load.folder));
        let webview = wry_page::builder(
            &mut context,
            Self::ORIGIN,
            load.dev,
            load.assets,
            load.receive,
        )
        // WebView2 opens a file dropped on it from outside in the page's
        // place. A drop handler turns that off, and this one takes the files
        // to nothing.
        .with_drag_drop_handler(|_| true)
        .with_bounds(bounds(size, scale))
        // The WebView starts hidden and is shown once it has started, because
        // wry leaves its window in the host's window when the WebView fails
        // to start, and a hidden window shows nothing there.
        .with_visible(false)
        .with_default_context_menus(load.devtools)
        .with_browser_accelerator_keys(load.devtools)
        .build_as_child(parent)
        .map_err(|error| format!("the WebView did not start: {error}"))?;
        // If showing fails, dropping `webview` destroys its window.
        webview
            .set_visible(true)
            .map_err(|error| format!("the WebView did not show: {error}"))?;
        Ok(WebView2(webview))
    }

    /// Returns the temporary folder, which is the user's own on Windows.
    fn fallback() -> Option<PathBuf> {
        Some(std::env::temp_dir())
    }

    fn evaluate(&self, script: &str) {
        let _ = self.0.evaluate_script(script);
    }

    fn place(&self, size: Size, scale: f64) {
        let _ = self.0.set_bounds(bounds(size, scale));
    }

    fn focus_host(&self) -> Result<(), String> {
        self.0.focus_parent().map_err(|error| error.to_string())
    }

    /// Passes on nothing, because the host's message loop dispatches
    /// WebView2's events, and the page's messages reach the editor from
    /// there. Returns `None`, because wry has no event for a WebView2 page
    /// that has gone.
    fn poll(&self) -> Option<String> {
        None
    }
}

/// Returns the WebView's bounds, which cover the host's window, in physical
/// pixels.
fn bounds(size: Size, scale: f64) -> Rect {
    let pixels = |logical: u32| (f64::from(logical) * scale).round() as u32;
    Rect {
        position: PhysicalPosition::new(0, 0).into(),
        size: PhysicalSize::new(pixels(size.width), pixels(size.height)).into(),
    }
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
