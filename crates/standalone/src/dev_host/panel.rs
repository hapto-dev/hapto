//! A panel of the dev host's: a WebView showing one of its pages, the bar
//! or the settings window's, in a rectangle of a window, and the messages
//! the page posts.

#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Context as _, Result};
use winit::window::Window;
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{NewWindowResponse, WebContext, WebView, WebViewBuilder};

use super::layout::Rect;
use super::message::Message;

/// A WebView of the dev host's and the messages its page posted since the
/// last look.
pub(crate) struct Panel {
    webview: WebView,
    inbox: Rc<RefCell<Vec<String>>>,
    /// Its browser data's folder, kept for the WebView's life.
    _context: WebContext,
}

impl Panel {
    /// Opens `page` in `window` at `rect`, keeping its browser data in
    /// `data`. The folder is the session's, not one beside the app: `cargo
    /// hapto dev` runs a copy of the app under a new name each time.
    ///
    /// # Errors
    ///
    /// Returns why the WebView did not open: no WebView2 runtime, say.
    pub(crate) fn open(window: &Window, page: &str, rect: Rect, data: PathBuf) -> Result<Panel> {
        let inbox = Rc::new(RefCell::new(Vec::new()));
        let posted = Rc::clone(&inbox);
        let mut context = WebContext::new(Some(data));
        let webview = WebViewBuilder::new_with_web_context(&mut context)
            .with_html(page)
            .with_bounds(bounds(rect))
            .with_focused(false)
            .with_ipc_handler(move |request| posted.borrow_mut().push(request.into_body()))
            // The page posts to the dev host, so no other page may take its
            // place: not a dropped file or link, nor a page a link opens.
            .with_navigation_handler(|url| is_own_page(&url))
            .with_drag_drop_handler(|_| true)
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
            .build_as_child(window)
            .context("the dev host's page")?;
        Ok(Panel {
            webview,
            inbox,
            _context: context,
        })
    }

    /// Moves it to `rect`, as its window resizes.
    pub(crate) fn place(&self, rect: Rect) {
        let _placed = self.webview.set_bounds(bounds(rect));
    }

    /// Returns the messages the page posted since the last call.
    pub(crate) fn messages(&self) -> Vec<Message> {
        std::mem::take(&mut *self.inbox.borrow_mut())
            .iter()
            .filter_map(|text| Message::read(text))
            .collect()
    }

    /// Runs `script` in the page: its `update`, with what it shows.
    pub(crate) fn run(&self, script: &str) {
        let _ran = self.webview.evaluate_script(script);
    }
}

fn bounds(rect: Rect) -> wry::Rect {
    wry::Rect {
        position: LogicalPosition::new(rect.x, rect.y).into(),
        size: LogicalSize::new(rect.width, rect.height).into(),
    }
}

/// Whether a navigation to `url` is to the panel's own page, which a panel
/// opens from its HTML: an `about:` or `data:` address, never a file or a
/// site.
fn is_own_page(url: &str) -> bool {
    url.starts_with("about:") || url.starts_with("data:")
}
