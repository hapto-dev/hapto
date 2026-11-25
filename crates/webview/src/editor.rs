//! The editor: a WebView child window inside the host's, made by wry. What
//! differs between the systems' web engines is behind
//! [`WebEngine`](crate::platform::WebEngine).
//!
//! On Windows, wry runs the thread's message loop until the WebView exists,
//! so the host's messages, a tick among them, can arrive inside `open`. The
//! exporter puts off what they ask of this editor until `open` returns. The
//! page's messages arrive later, from the message loop, through the IPC
//! handler, outside any call of the exporter's. A panic there is caught,
//! because it would otherwise unwind into the WebView.
//!
//! The keyboard: the WebView takes it when the page is clicked, and the page
//! gives it back when nothing there needs keys, with one fixed message,
//! [`HAND_BACK`], below the protocol. The shell then focuses the host's
//! window, so a key such as the space bar reaches the host again. A
//! hand-back is dropped while a command runs, since a dialog's message loop
//! may be what has the keyboard then.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::error::Error;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Instant;

use hapto_bridge::{Dispatcher, Outbox};
use hapto_host::{Editor, ParentWindow, Size};
use wry::dpi::{PhysicalPosition, PhysicalSize};
use wry::http::header::{CONTENT_TYPE, HeaderValue};
use wry::http::{Response, StatusCode};
use wry::{Rect, WebContext, WebView, WebViewBuilder};

use crate::Page;
use crate::assets::{SCHEME, Served, dev_url, is_page, page_url, serve};
use crate::native::{HAND_BACK, Native, script};
use crate::platform::{Engine, WebEngine as _};

/// The page's side of the conversation: what the dispatcher posts goes to
/// the WebView open now, in order, and nowhere while none is.
struct ToPage {
    name: &'static str,
    webview: Rc<RefCell<Weak<WebView>>>,
    /// When the editor opened, until the page's first `ready` of that open
    /// is answered.
    opened: Rc<Cell<Option<Instant>>>,
}

impl Outbox for ToPage {
    fn post(&mut self, message: &str) {
        if message.starts_with(r#"{"type":"snapshot""#)
            && let Some(opened) = self.opened.take()
        {
            hapto_host::report_line(&format!(
                "hapto: {}: the page answered its first ready {} ms after the editor opened.",
                self.name,
                opened.elapsed().as_millis()
            ));
        }
        let webview = self.webview.borrow().upgrade();
        if let Some(webview) = webview {
            // A message the page cannot take is lost with the page, which
            // asks again with its next `ready`.
            let _ = webview.evaluate_script(&script(message));
        }
    }
}

/// The editor: one WebView at a time, made at each open.
pub(crate) struct PageEditor {
    name: &'static str,
    folder: PathBuf,
    assets: &'static [crate::Asset],
    size: Size,
    scale: f64,
    native: Rc<Native<ToPage>>,
    target: Rc<RefCell<Weak<WebView>>>,
    opened: Rc<Cell<Option<Instant>>>,
    webview: Option<Rc<WebView>>,
}

impl PageEditor {
    pub(crate) fn new(page: Page) -> Self {
        let target = Rc::new(RefCell::new(Weak::new()));
        let opened = Rc::new(Cell::new(None));
        let outbox = ToPage {
            name: page.name,
            webview: Rc::clone(&target),
            opened: Rc::clone(&opened),
        };
        let dispatcher = Dispatcher::new(page.ui, page.commands, page.values, outbox);
        PageEditor {
            name: page.name,
            folder: Engine::data_folder(page.id),
            assets: page.assets,
            size: page.size,
            scale: 1.0,
            native: Rc::new(Native::new(dispatcher)),
            target,
            opened,
            webview: None,
        }
    }

    /// The WebView's place in the host's window: all of it, in the screen's
    /// pixels.
    fn bounds(&self) -> Rect {
        let pixels = |logical: u32| (f64::from(logical) * self.scale).round() as u32;
        Rect {
            position: PhysicalPosition::new(0, 0).into(),
            size: PhysicalSize::new(pixels(self.size.width), pixels(self.size.height)).into(),
        }
    }

    fn place(&self) {
        if let Some(webview) = &self.webview {
            // A WebView whose window went with the host's takes no bounds;
            // the close that follows drops it.
            let _ = webview.set_bounds(self.bounds());
        }
    }
}

impl Editor for PageEditor {
    fn size(&self) -> Size {
        self.size
    }

    fn open(&mut self, parent: &ParentWindow) -> Result<(), Box<dyn Error>> {
        Engine::prepare()?;
        self.opened.set(Some(Instant::now()));
        let mut context = WebContext::new(Some(self.folder.clone()));
        let (assets, name) = (self.assets, self.name);
        let native = Rc::clone(&self.native);
        let target = Rc::clone(&self.target);
        let dev = dev_url();
        let (navigable, speaks) = (dev.clone(), dev.clone());
        let builder = WebViewBuilder::new_with_web_context(&mut context)
            .with_url(page_url(dev))
            .with_custom_protocol(SCHEME.to_owned(), move |_, request| {
                respond(assets, request.uri().path())
            })
            .with_ipc_handler(move |request| {
                // Only the page speaks for the user: any other document's
                // message is dropped.
                if is_page(
                    &request.uri().to_string(),
                    Engine::ORIGIN,
                    speaks.as_deref(),
                ) {
                    from_page(name, &native, &target, request.into_body());
                }
            })
            // The WebView shows the page and nothing else: a link or a
            // script cannot take it elsewhere.
            .with_navigation_handler(move |url| is_page(&url, Engine::ORIGIN, navigable.as_deref()))
            // WebView2 opens a file dropped on it from outside in the page's
            // place. A drop handler turns that off, and this one takes the
            // files to nothing.
            .with_drag_drop_handler(|_| true)
            .with_bounds(self.bounds())
            // Shown once it started: wry leaves its window in the host's
            // when the WebView fails to start, and hidden, it shows nothing
            // there until the host's window goes.
            .with_visible(false)
            // The page takes the keyboard when it is clicked, not as the
            // window opens, so the host keeps it until then.
            .with_focused(false);
        let built = Engine::configure(builder).build_as_child(parent);
        let shown = built
            .map_err(|error| format!("the WebView did not start: {error}"))
            .and_then(|webview| match webview.set_visible(true) {
                Ok(()) => Ok(webview),
                // Dropped, it destroys its window.
                Err(error) => Err(format!("the WebView did not show: {error}")),
            });
        match shown {
            Ok(webview) => {
                let webview = Rc::new(webview);
                *self.target.borrow_mut() = Rc::downgrade(&webview);
                self.webview = Some(webview);
                Ok(())
            }
            Err(reason) => {
                self.opened.set(None);
                Err(reason.into())
            }
        }
    }

    fn close(&mut self) {
        self.native.end_session();
        self.opened.set(None);
        *self.target.borrow_mut() = Weak::new();
        // Dropping it closes the WebView and destroys its window.
        self.webview = None;
    }

    fn set_size(&mut self, size: Size) {
        self.size = size;
        self.place();
    }

    fn set_scale(&mut self, scale: f64) {
        self.scale = scale;
        self.place();
    }

    fn tick(&mut self) {
        Engine::poll();
        self.native.tick();
    }
}

/// Takes a message the page posted: the hand-back, or one for the
/// dispatcher.
fn from_page(
    name: &'static str,
    native: &Native<ToPage>,
    target: &RefCell<Weak<WebView>>,
    message: String,
) {
    if message != HAND_BACK {
        deliver(name, native, message);
        return;
    }
    if hand_back(native, target) == HandBack::Panicked {
        hapto_host::report_line(&format!(
            "hapto: {name}: giving the keyboard back panicked and was dropped."
        ));
    }
}

/// What a hand-back did.
#[derive(Debug, PartialEq)]
enum HandBack {
    /// The host's window was given the keyboard.
    Handed,
    /// Dropped: a command is running.
    InCall,
    /// No WebView is open.
    Closed,
    /// wry could not focus the host's window. Ignored, as a failed
    /// `set_bounds` is: the next click on the page asks again.
    Failed,
    /// It panicked, and was dropped.
    Panicked,
}

/// Gives the keyboard to the host's window, the one the WebView was built
/// in, unless a command is running or no WebView is open. It runs inside
/// the WebView's callback, so a panic is caught here.
fn hand_back(native: &Native<ToPage>, target: &RefCell<Weak<WebView>>) -> HandBack {
    let handed = panic::catch_unwind(AssertUnwindSafe(|| {
        if native.in_call() {
            return HandBack::InCall;
        }
        let Some(webview) = target.borrow().upgrade() else {
            return HandBack::Closed;
        };
        match webview.focus_parent() {
            Ok(()) => HandBack::Handed,
            Err(_) => HandBack::Failed,
        }
    }));
    handed.unwrap_or(HandBack::Panicked)
}

/// Hands a message the page posted to the native side. It runs inside
/// the WebView's callback, so a panic is caught here and the message dropped.
fn deliver(name: &'static str, native: &Native<ToPage>, message: String) {
    let handled = panic::catch_unwind(AssertUnwindSafe(|| native.deliver(message)));
    if handled.is_err() {
        hapto_host::report_line(&format!(
            "hapto: {name}: a message from the page panicked and was dropped."
        ));
    }
}

/// The scheme's answer to a request for `path`, built from the page's
/// files with no copy.
fn respond(assets: &'static [crate::Asset], path: &str) -> Response<Cow<'static, [u8]>> {
    answer(|| serve(assets, path))
}

/// What a request is answered when serving it panicked.
const FAILED: Served = Served {
    status: 500,
    content_type: "text/plain; charset=utf-8",
    body: b"the file could not be served",
};

/// The response for what `serve` gives, or 500 when it panics: it runs
/// inside the WebView's callback, where a panic must not unwind.
fn answer(serve: impl FnOnce() -> Served) -> Response<Cow<'static, [u8]>> {
    let served = panic::catch_unwind(AssertUnwindSafe(serve)).unwrap_or(FAILED);
    let mut response = Response::new(Cow::Borrowed(served.body));
    *response.status_mut() = StatusCode::from_u16(served.status).unwrap_or(StatusCode::NOT_FOUND);
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(served.content_type));
    response
}

impl std::fmt::Debug for PageEditor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageEditor")
            .field("plugin", &self.name)
            .field("size", &self.size)
            .field("open", &self.webview.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use hapto_bridge::Refusals;
    use hapto_params::{ParamId, UiParams};

    use super::*;
    use crate::fixtures::{DIALOG, INFO, PAGE, READY, dialog, during_dialog, params};

    /// An editor never opened, over one parameter, with the `dialog`
    /// command.
    fn editor() -> (PageEditor, UiParams, ParamId) {
        let (_, ui, bypass) = params();
        let page = Page::new(&INFO, ui.clone(), PAGE).command("dialog", dialog);
        (PageEditor::new(page), ui, bypass)
    }

    /// Posts as the page does, through the WebView's IPC handler.
    fn poster(editor: &PageEditor) -> impl Fn(&str) + 'static {
        let native = Rc::clone(&editor.native);
        let target = Rc::clone(&editor.target);
        move |message| from_page("Panel", &native, &target, message.to_owned())
    }

    #[test]
    fn closing_the_editor_ends_the_pages_session() {
        let (mut editor, ui, bypass) = editor();
        let post = poster(&editor);
        post(READY);
        post(r#"{"type":"gestureBegin","param":"bypass"}"#);
        assert!(ui.is_open(bypass));
        editor.close();
        assert!(!ui.is_open(bypass), "the page's drag ends for the host");
    }

    #[test]
    fn the_hand_back_reaches_no_dispatcher_and_any_other_text_does() {
        let (editor, _, _) = editor();
        let post = poster(&editor);
        post(READY);
        post(HAND_BACK);
        assert_eq!(
            editor.native.refusals(),
            Refusals::default(),
            "the hand-back is not refused as malformed"
        );
        post(r#"{"shell":"releaseKeys","more":1}"#);
        assert_eq!(
            editor.native.refusals().malformed,
            1,
            "only the exact text is the hand-back"
        );
    }

    #[test]
    fn a_hand_back_with_no_webview_open_does_nothing() {
        let (editor, _, _) = editor();
        assert_eq!(hand_back(&editor.native, &editor.target), HandBack::Closed);
    }

    #[test]
    fn a_hand_back_while_a_command_runs_is_dropped() {
        let (editor, _, _) = editor();
        let post = poster(&editor);
        let seen = Rc::new(RefCell::new(None));
        let (native, target) = (Rc::clone(&editor.native), Rc::clone(&editor.target));
        let during = Rc::clone(&seen);
        during_dialog(move || *during.borrow_mut() = Some(hand_back(&native, &target)));
        post(READY);
        post(DIALOG);
        assert_eq!(*seen.borrow(), Some(HandBack::InCall));
    }

    #[test]
    fn a_file_is_served_and_a_panic_while_serving_answers_500() {
        let served = respond(PAGE, "/index.html");
        assert_eq!(served.status(), StatusCode::OK);
        assert_eq!(served.body().as_ref(), b"<!doctype html>");
        let failed = answer(|| panic!("a bug while serving"));
        assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            failed.headers().get(CONTENT_TYPE),
            Some(&HeaderValue::from_static("text/plain; charset=utf-8"))
        );
    }
}
