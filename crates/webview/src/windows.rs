//! The editor on Windows: a WebView2 child window inside the host's, made
//! by wry.
//!
//! wry's start-up runs the thread's message loop until the WebView exists,
//! so the host's messages, a tick among them, arrive inside `open`; the
//! exporter puts off what they ask of this editor until `open` returns. The
//! page's messages arrive later, from the message loop, through the IPC
//! handler, outside any call of the exporter's: a panic there is caught,
//! since it would unwind into WebView2.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::error::Error;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Instant;

use seam_bridge::{Dispatcher, Outbox};
use seam_host::{Editor, ParentWindow, Size};
use wry::dpi::{PhysicalPosition, PhysicalSize};
use wry::http::header::{CONTENT_TYPE, HeaderValue};
use wry::http::{Response, StatusCode};
use wry::{Rect, WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows};

use crate::Page;
use crate::assets::{SCHEME, Served, data_folder, dev_url, is_page, page_url, serve};

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
            seam_host::report_line(&format!(
                "seam: {}: the page answered its first ready {} ms after the editor opened.",
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

/// The script that hands `message` to the page's transport, which listens
/// as `window.__seam`: the message as a JavaScript string, which JSON's
/// string syntax is.
fn script(message: &str) -> String {
    let text = serde_json::Value::String(message.to_owned()).to_string();
    format!("window.__seam?.receive({text})")
}

/// The page's native side, which the editor and its WebView's IPC handler
/// share: the dispatcher, and what waits while it is in a call.
///
/// A command the page calls may run a message loop of its own, such as a
/// file dialog's, and the page's next messages and the host's close then
/// arrive inside the call. A message is held and handed over once the call
/// returns, before any that comes later, and a close's end of the session
/// is done then.
struct Native {
    dispatcher: RefCell<Dispatcher<ToPage>>,
    /// Messages the page sent while the dispatcher was in a call, in order.
    held: RefCell<VecDeque<String>>,
    /// Whether the page's session ended while the dispatcher was in a call.
    ending: Cell<bool>,
}

impl Native {
    /// Hands `message` to the dispatcher after those held before it, or
    /// holds it while the dispatcher is in a call.
    fn deliver(&self, message: String) {
        self.held.borrow_mut().push_back(message);
        if let Ok(mut dispatcher) = self.dispatcher.try_borrow_mut() {
            self.catch_up(&mut dispatcher);
        }
    }

    /// Hands over what was held, then tells the page what the tick changed;
    /// nothing while the dispatcher is in a call.
    fn tick(&self) {
        if let Ok(mut dispatcher) = self.dispatcher.try_borrow_mut() {
            self.catch_up(&mut dispatcher);
            dispatcher.tick();
        }
    }

    /// Ends the page's session and drops what it sent that is still held:
    /// at once, or as the call the dispatcher is in returns.
    fn end_session(&self) {
        self.held.borrow_mut().clear();
        match self.dispatcher.try_borrow_mut() {
            Ok(mut dispatcher) => dispatcher.end_session(),
            Err(_) => self.ending.set(true),
        }
    }

    /// Does what waited for the dispatcher, in order: a session's end, then
    /// each held message, whose call may bring more of either.
    fn catch_up(&self, dispatcher: &mut Dispatcher<ToPage>) {
        loop {
            if self.ending.take() {
                dispatcher.end_session();
            }
            let Some(message) = self.held.borrow_mut().pop_front() else {
                return;
            };
            dispatcher.receive(&message);
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
    native: Rc<Native>,
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
        let base = std::env::var_os("LOCALAPPDATA").map_or_else(std::env::temp_dir, PathBuf::from);
        PageEditor {
            name: page.name,
            folder: data_folder(&base, page.id),
            assets: page.assets,
            size: page.size,
            scale: 1.0,
            native: Rc::new(Native {
                dispatcher: RefCell::new(Dispatcher::new(page.ui, page.commands, outbox)),
                held: RefCell::new(VecDeque::new()),
                ending: Cell::new(false),
            }),
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
        self.opened.set(Some(Instant::now()));
        let mut context = WebContext::new(Some(self.folder.clone()));
        let (assets, name) = (self.assets, self.name);
        let native = Rc::clone(&self.native);
        let dev = dev_url();
        let (navigable, speaks) = (dev.clone(), dev.clone());
        let built = WebViewBuilder::new_with_web_context(&mut context)
            .with_url(page_url(dev))
            .with_custom_protocol(SCHEME.to_owned(), move |_, request| {
                respond(assets, request.uri().path())
            })
            .with_ipc_handler(move |request| {
                // Only the page speaks for the user: any other document's
                // message is dropped.
                if is_page(&request.uri().to_string(), speaks.as_deref()) {
                    deliver(name, &native, request.into_body());
                }
            })
            // The WebView shows the page and nothing else: a link or a
            // script cannot take it elsewhere.
            .with_navigation_handler(move |url| is_page(&url, navigable.as_deref()))
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
            .with_focused(false)
            // A release build shows no browser menu and takes no browser
            // shortcut, such as a reload.
            .with_default_context_menus(cfg!(debug_assertions))
            .with_browser_accelerator_keys(cfg!(debug_assertions))
            .build_as_child(parent);
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
        self.native.tick();
    }
}

/// Hands a message the page posted to the native side. It runs inside
/// WebView2's callback, so a panic is caught here and the message dropped.
fn deliver(name: &'static str, native: &Native, message: String) {
    let handled = panic::catch_unwind(AssertUnwindSafe(|| native.deliver(message)));
    if handled.is_err() {
        seam_host::report_line(&format!(
            "seam: {name}: a message from the page panicked and was dropped."
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
/// inside WebView2's callback, where a panic must not unwind.
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

    use seam_host::{AuCodes, Category, Layout, PluginInfo};
    use seam_params::{Kind, ParamDecl, ParamId, ParamSet, UiParams};

    use super::*;
    use crate::{Asset, Page};

    const INFO: PluginInfo = PluginInfo {
        name: "Panel",
        vendor: "Example",
        url: "",
        email: "",
        description: "",
        version: "0.1.0",
        id: "dev.example.panel",
        vst3_class_id: [0; 16],
        au_codes: AuCodes {
            subtype: *b"panl",
            manufacturer: *b"Exmp",
        },
        category: Category::Effect,
        features: &[],
        layouts: &[Layout::STEREO],
        midi_input: false,
        midi_output: false,
    };

    const PAGE: &[Asset] = &[Asset {
        path: "index.html",
        bytes: b"<!doctype html>",
    }];

    const READY: &str = r#"{"type":"ready","protocol":2}"#;
    const DIALOG: &str = r#"{"type":"invoke","call":1,"command":"dialog","args":null}"#;

    thread_local! {
        /// What arrives while the page's `dialog` command runs, as a file
        /// dialog's message loop would deliver it.
        static DURING_DIALOG: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
    }

    /// An editor never opened, over one parameter, with a command that runs
    /// what `DURING_DIALOG` holds.
    fn editor() -> (Rc<RefCell<PageEditor>>, UiParams, ParamId) {
        let set = ParamSet::from_decls([ParamDecl::new("bypass", "Bypass", Kind::Bool, 0.0)])
            .expect("valid declarations");
        let bypass = set.id("bypass").expect("declared");
        let ui = set.take_ui().expect("the first UI handle");
        let page = Page::new(&INFO, ui.clone(), PAGE).command("dialog", |()| {
            DURING_DIALOG.with(|during| {
                if let Some(during) = &*during.borrow() {
                    during();
                }
            });
            Ok(())
        });
        (Rc::new(RefCell::new(PageEditor::new(page))), ui, bypass)
    }

    /// Posts as the page does, through the WebView's IPC handler.
    fn poster(editor: &PageEditor) -> impl Fn(&str) + 'static {
        let native = Rc::clone(&editor.native);
        move |message| deliver("Panel", &native, message.to_owned())
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

    #[test]
    fn a_message_held_while_a_command_runs_keeps_its_place() {
        let (editor, ui, bypass) = editor();
        let post = poster(&editor.borrow());
        let during = poster(&editor.borrow());
        DURING_DIALOG.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                during(r#"{"type":"set","param":"bypass","normalized":1}"#);
            }));
        });
        post(READY);
        post(DIALOG);
        post(r#"{"type":"set","param":"bypass","normalized":0}"#);
        editor.borrow_mut().tick();
        assert_eq!(ui.value(bypass), 0.0, "the later set lands last");
    }

    #[test]
    fn a_close_while_a_command_runs_ends_the_session_as_it_returns() {
        let (editor, ui, bypass) = editor();
        let post = poster(&editor.borrow());
        let closing = Rc::downgrade(&editor);
        DURING_DIALOG.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                if let Some(editor) = closing.upgrade() {
                    editor.borrow_mut().close();
                }
            }));
        });
        post(READY);
        post(r#"{"type":"gestureBegin","param":"bypass"}"#);
        assert!(ui.is_open(bypass));
        post(DIALOG);
        assert!(!ui.is_open(bypass), "the page's drag ends for the host");
    }
}
