//! The editor: the page in the host's window, opened by the system's web
//! engine. What differs between the engines is behind
//! [`WebEngine`](crate::platform::WebEngine).
//!
//! On Windows, wry runs the thread's message loop until the WebView exists,
//! so the host's messages, a tick among them, can arrive inside `open`. The
//! exporter puts off what they ask of this editor until `open` returns. The
//! page's messages arrive later. On Windows they come from the message loop,
//! outside any call of the exporter's, and on Linux they come during the
//! editor's tick. The editor catches a panic while it handles one or a
//! hand-back, because on Windows the panic would otherwise unwind into the
//! WebView.
//!
//! The keyboard: the WebView takes it when the page is clicked, and the page
//! gives it back when nothing there needs keys, with one fixed message,
//! [`HAND_BACK`], below the protocol. The shell then focuses the host's
//! window, so a key such as the space bar reaches the host again. A
//! hand-back is dropped while a command runs, since a dialog's message loop
//! may be what has the keyboard then.

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::time::Instant;

use hapto_bridge::{Dispatcher, Outbox};
use hapto_host::{Editor, NoDataFolder, ParentWindow, Size};

use crate::Page;
use crate::assets::dev_url;
use crate::native::{HAND_BACK, Native, script};
use crate::platform::{Engine, Load, WebEngine as _};

/// The page's side of the conversation: what the dispatcher posts goes to
/// the WebView open now, in order, and nowhere while none is.
struct ToPage {
    name: &'static str,
    target: Rc<RefCell<Weak<Engine>>>,
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
        let view = self.target.borrow().upgrade();
        if let Some(view) = view {
            view.evaluate(&script(message));
        }
    }
}

/// The editor: one WebView at a time, made at each open.
pub(crate) struct PageEditor {
    name: &'static str,
    folder: Result<PathBuf, NoDataFolder>,
    assets: &'static [crate::Asset],
    size: Size,
    scale: f64,
    native: Rc<Native<ToPage>>,
    target: Rc<RefCell<Weak<Engine>>>,
    opened: Rc<Cell<Option<Instant>>>,
    view: Option<Rc<Engine>>,
    #[cfg(target_os = "linux")]
    accelerated: bool,
}

impl PageEditor {
    pub(crate) fn new(page: Page) -> Self {
        let target = Rc::new(RefCell::new(Weak::new()));
        let opened = Rc::new(Cell::new(None));
        let outbox = ToPage {
            name: page.name,
            target: Rc::clone(&target),
            opened: Rc::clone(&opened),
        };
        let dispatcher = Dispatcher::new(page.ui, page.commands, page.values, outbox);
        PageEditor {
            name: page.name,
            folder: engine_folder(page.id),
            assets: page.assets,
            size: page.size,
            scale: 1.0,
            native: Rc::new(Native::new(dispatcher)),
            target,
            opened,
            view: None,
            #[cfg(target_os = "linux")]
            accelerated: page.accelerated,
        }
    }

    fn place(&self) {
        if let Some(view) = &self.view {
            view.place(self.size, self.scale);
        }
    }

    /// Creates the engine's data folder if it is missing, and returns it. The
    /// editor creates it, rather than the engine, so a folder that can't be
    /// created fails the open the same way with every engine.
    ///
    /// # Errors
    ///
    /// Returns why the folder is unknown or couldn't be made.
    fn create_folder(&self) -> Result<PathBuf, String> {
        let folder = self
            .folder
            .as_ref()
            .map_err(|unknown| format!("the WebView has no folder for its data: {unknown}"))?;
        std::fs::create_dir_all(folder).map_err(|error| {
            format!(
                "the WebView's data folder {} could not be made: {error}",
                folder.display()
            )
        })?;
        Ok(folder.clone())
    }
}

impl Editor for PageEditor {
    fn size(&self) -> Size {
        self.size
    }

    fn open(&mut self, parent: &ParentWindow) -> Result<(), Box<dyn Error>> {
        let folder = self.create_folder()?;
        self.opened.set(Some(Instant::now()));
        let (name, native, target) = (self.name, Rc::clone(&self.native), Rc::clone(&self.target));
        let load = Load {
            dev: dev_url(),
            folder,
            assets: self.assets,
            receive: Box::new(move |message| from_page(name, &native, &target, message)),
            devtools: cfg!(debug_assertions),
            #[cfg(target_os = "linux")]
            accelerated: self.accelerated,
        };
        match Engine::open(load, parent, self.size, self.scale) {
            Ok(view) => {
                let view = Rc::new(view);
                *self.target.borrow_mut() = Rc::downgrade(&view);
                self.view = Some(view);
                Ok(())
            }
            Err(reason) => {
                self.opened.set(None);
                Err(reason.into())
            }
        }
    }

    fn close(&mut self) {
        *self.target.borrow_mut() = Weak::new();
        // The view drops before the session ends, because dropping it
        // delivers the messages that the page posted before it closed.
        self.view = None;
        self.native.end_session();
        self.opened.set(None);
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
        if let Some(gone) = self.view.as_ref().and_then(|view| view.poll()) {
            hapto_host::report_line(&format!(
                "hapto: {}: the page is gone, because {gone}.",
                self.name
            ));
        }
        self.native.tick();
    }
}

/// Returns the folder where the engine keeps its data for the plugin whose
/// id is `id`, which every instance of the plugin shares. It is the engine's
/// own folder in the plugin's data folder, or below the engine's fallback
/// when the user's data folder is unknown.
///
/// # Errors
///
/// Returns why the user's data folder is unknown, when the engine has no
/// fallback either.
fn engine_folder(id: &str) -> Result<PathBuf, NoDataFolder> {
    let plugin = hapto_host::data_folder(id).or_else(|unknown| {
        Engine::fallback()
            .map(|base| hapto_host::plugin_folder(&base, id))
            .ok_or(unknown)
    })?;
    Ok(plugin.join(Engine::FOLDER))
}

/// Takes a message the page posted: the hand-back, or one for the
/// dispatcher.
fn from_page(
    name: &'static str,
    native: &Native<ToPage>,
    target: &RefCell<Weak<Engine>>,
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
    /// The engine couldn't focus the host's window. The editor ignores this,
    /// as it ignores a failed placement, because the next click sends another
    /// hand-back.
    Failed,
    /// It panicked, and was dropped.
    Panicked,
}

/// Gives the keyboard to the host's window, the one the WebView was built
/// in, unless a command is running or no WebView is open. It catches a panic,
/// for the reason that the module's docs give.
fn hand_back(native: &Native<ToPage>, target: &RefCell<Weak<Engine>>) -> HandBack {
    let handed = panic::catch_unwind(AssertUnwindSafe(|| {
        if native.in_call() {
            return HandBack::InCall;
        }
        let Some(view) = target.borrow().upgrade() else {
            return HandBack::Closed;
        };
        match view.focus_host() {
            Ok(()) => HandBack::Handed,
            Err(_) => HandBack::Failed,
        }
    }));
    handed.unwrap_or(HandBack::Panicked)
}

/// Hands a message the page posted to the native side. It catches a panic,
/// for the reason that the module's docs give, and drops the message.
fn deliver(name: &'static str, native: &Native<ToPage>, message: String) {
    let handled = panic::catch_unwind(AssertUnwindSafe(|| native.deliver(message)));
    if handled.is_err() {
        hapto_host::report_line(&format!(
            "hapto: {name}: a message from the page panicked and was dropped."
        ));
    }
}

impl std::fmt::Debug for PageEditor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageEditor")
            .field("plugin", &self.name)
            .field("size", &self.size)
            .field("open", &self.view.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::Path;
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
    fn the_engine_keeps_its_data_in_its_own_folder_in_the_plugins() {
        let folder = engine_folder("a/b").expect("a folder");
        assert!(
            folder.ends_with(Path::new("a_b").join(Engine::FOLDER)),
            "{}",
            folder.display()
        );
    }
}
