//! The page's helper process on Linux. Each page is in a GTK plug inside its
//! host window, because a WebView that wry puts in an X11 window itself
//! (`build_as_child`) ends the process when it closes.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt::Display;
use std::fs::{DirBuilder, File, TryLockError};
use std::io::{self, BufReader, Read as _, Write as _};
use std::num::NonZeroU32;
use std::os::fd::{AsFd as _, AsRawFd as _};
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::DirBuilderExt as _;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{SystemTime, UNIX_EPOCH};

use gtk::gdk::EventMask;
use gtk::glib::{self, ControlFlow, IOCondition, WeakRef};
use gtk::prelude::{
    EventControllerExt as _, GtkWindowExt as _, ObjectExt as _, PlugExt as _, WidgetExt as _,
};
use wry::{WebContext, WebView, WebViewBuilderExtUnix as _};
use x11rb::connection::Connection as _;
use x11rb::cookie::VoidCookie;
use x11rb::protocol::xproto::{self, ClientMessageEvent, ConnectionExt as _};
use x11rb::rust_connection::RustConnection;

use crate::Asset;
use crate::wire::{
    self, Frame as _, FromHelper, HELPER, Open, PageId, Place, Request, Setup, Start,
};
use crate::wry_page;

/// The origin that WebKitGTK serves the page's scheme at.
const ORIGIN: &str = "hapto://localhost";

/// The file in a data folder that the helper using it locks.
const LOCK: &str = "hapto-webview.lock";

/// The start of a temporary data folder's name, which the helper's process
/// id follows.
const TEMPORARY: &str = "hapto-webview-";

/// A script that cancels the default action of a right click, so WebKitGTK
/// shows no menu of its own. A listener of the page's own still runs.
const NO_CONTEXT_MENU: &str =
    "document.addEventListener('contextmenu', (event) => event.preventDefault(), true);";

/// Runs the helper. It reads what every page of the plugin shares and the
/// page's files from the editors' socket, starts GTK, and runs GTK's main
/// loop, which opens and closes pages as the editors ask, until they shut
/// the socket.
pub fn run() -> ExitCode {
    let (to_editor, from_editor) = match connect() {
        Ok(connected) => connected,
        Err(error) => {
            eprintln!("{HELPER}: standard input isn't a socket from an editor: {error}");
            return ExitCode::FAILURE;
        }
    };
    match serve(&to_editor, from_editor) {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            to_editor.send(&FromHelper::Stopped(reason));
            ExitCode::FAILURE
        }
    }
}

/// Returns two handles to the editors' socket, which is standard input: one
/// for writing and one for reading. Each is a duplicate with close-on-exec
/// set, and GLib starts the helper's children with `/dev/null` as their
/// standard input, so no child holds the socket.
///
/// # Errors
///
/// Returns why standard input isn't a socket that the helper can use.
fn connect() -> io::Result<(ToEditor, BufReader<UnixStream>)> {
    let socket = UnixStream::from(io::stdin().as_fd().try_clone_to_owned()?);
    // Fails unless standard input is a socket, as the editor makes it.
    socket.local_addr()?;
    let reading = socket.try_clone()?;
    Ok((ToEditor(Rc::new(socket)), BufReader::new(reading)))
}

/// Starts the pages from what the editor sent, and runs GTK's main loop until
/// the editors shut the socket.
///
/// # Errors
///
/// Returns why the helper couldn't start.
fn serve(to_editor: &ToEditor, mut from_editor: BufReader<UnixStream>) -> Result<(), String> {
    let (start, assets) = read_setup(&mut from_editor)?;
    let data = DataFolder::new(PathBuf::from(OsString::from_vec(start.folder)))?;
    start_gtk()?;
    let keyboard = Rc::new(Keyboard::connect()?);
    // The reader thread writes a byte to `wake` after each request, so GTK's
    // main loop sleeps until there is one.
    let failed = |error: io::Error| format!("couldn't wake the helper's main loop: {error}");
    let (wake, woken) = UnixStream::pair().map_err(failed)?;
    wake.set_nonblocking(true).map_err(failed)?;
    woken.set_nonblocking(true).map_err(failed)?;
    let (requests, reader) = wire::read_on_a_thread(HELPER, from_editor, move || {
        // A full socket already wakes the main loop.
        let _ = (&wake).write(&[0]);
    })
    .map_err(|error| format!("the thread that reads the editors didn't start: {error}"))?;
    let pages = Pages {
        pages: HashMap::new(),
        context: WebContext::new(Some(data.path().to_owned())),
        assets,
        dev: start.dev,
        devtools: start.devtools,
        keyboard,
        to_editor: to_editor.clone(),
        requests,
    };
    pages.run(woken);
    drop(data);
    if let Ok(Err(error)) = reader.join() {
        eprintln!("{HELPER}: couldn't read what the editors sent: {error}");
    }
    Ok(())
}

/// The handle to the editors' socket that the helper writes its frames to.
///
/// Thread: GTK's main thread, the only one that sends.
#[derive(Clone)]
struct ToEditor(Rc<UnixStream>);

impl ToEditor {
    /// Sends `message`, and ignores a failed write, because a write fails
    /// only once the editors have gone, and the helper then exits when it
    /// reads the end of the socket.
    fn send(&self, message: &FromHelper) {
        let _ = message.write(&mut &*self.0);
    }
}

/// Reads what every page of the plugin shares, then the page's files. The
/// files live for as long as the process, because the helper serves one
/// plugin's page and then exits.
///
/// # Errors
///
/// Returns why the editor's frames couldn't be read.
fn read_setup(
    from_editor: &mut BufReader<UnixStream>,
) -> Result<(Start, &'static [Asset]), String> {
    let failed = |error: io::Error| format!("couldn't read what the editor sent: {error}");
    let Some(Setup::Start(start)) = Setup::read(from_editor).map_err(failed)? else {
        return Err("the editor didn't send what the pages share first".to_owned());
    };
    let files = (0..start.files)
        .map(|_| match Setup::read(from_editor).map_err(failed)? {
            Some(Setup::File { path, bytes }) => Ok(asset(path, bytes)),
            _ => Err("the editor sent fewer of the page's files than it said".to_owned()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((start, Box::leak(files.into_boxed_slice())))
}

/// Returns one of the page's files, kept for as long as the process.
fn asset(path: Cow<'static, str>, bytes: Cow<'static, [u8]>) -> Asset {
    Asset {
        path: Box::leak(path.into_owned().into_boxed_str()),
        bytes: Box::leak(bytes.into_owned().into_boxed_slice()),
    }
}

/// The folder where the helper's pages keep their data, and the lock that
/// the helper and WebKitGTK's own processes hold on it. WebKitGTK expects one
/// process alone to use a data folder, so a helper locks the plugin's folder.
/// One that can't, as when two hosts show the same plugin's page at once,
/// keeps its pages' data in a temporary folder of its own instead, which it
/// removes as it exits. WebKitGTK's network process may still write to that
/// folder after the helper has exited, so the next helper to make a
/// temporary folder removes those that no process holds.
enum DataFolder {
    Plugin { folder: PathBuf, _lock: File },
    Temporary { folder: PathBuf, _lock: File },
}

impl DataFolder {
    /// Locks the plugin's data folder `folder`, or makes a temporary folder
    /// when it can't.
    ///
    /// # Errors
    ///
    /// Returns why neither folder could be used.
    fn new(folder: PathBuf) -> Result<DataFolder, String> {
        let refused = match lock(&folder) {
            Ok(lock) => {
                return Ok(DataFolder::Plugin {
                    folder,
                    _lock: lock,
                });
            }
            Err(refused) => refused,
        };
        let (temporary, lock) = temporary_folder()?;
        eprintln!(
            "{HELPER}: {refused}, so these pages keep their data in {} until they close",
            temporary.display()
        );
        Ok(DataFolder::Temporary {
            folder: temporary,
            _lock: lock,
        })
    }

    fn path(&self) -> &Path {
        match self {
            DataFolder::Plugin { folder, .. } | DataFolder::Temporary { folder, .. } => folder,
        }
    }
}

impl Drop for DataFolder {
    fn drop(&mut self) {
        if let DataFolder::Temporary { folder, .. } = self {
            let _ = std::fs::remove_dir_all(folder);
        }
    }
}

/// Locks `folder` for this process and the processes that it starts. The
/// lock goes when the last of them exits, however it exits, because each
/// holds the returned file open. WebKitGTK's network process exits only
/// after the helper, and writes to the folder as it does.
///
/// # Errors
///
/// Returns why the folder couldn't be locked: another process holds it, or
/// its lock's file couldn't be opened or locked.
fn lock(folder: &Path) -> Result<File, String> {
    let path = folder.join(LOCK);
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| format!("couldn't open {}: {error}", path.display()))?;
    match file.try_lock() {
        Ok(()) => {
            // Without close-on-exec, WebKitGTK's processes, which GLib starts
            // with the helper's other descriptors, keep the file open. A lock
            // that only this process holds is the next best thing.
            let _ = rustix::io::fcntl_setfd(&file, rustix::io::FdFlags::empty());
            Ok(file)
        }
        Err(TryLockError::WouldBlock) => Err(format!(
            "another process uses the data folder {}",
            folder.display()
        )),
        Err(TryLockError::Error(error)) => {
            Err(format!("couldn't lock {}: {error}", path.display()))
        }
    }
}

/// Makes and locks a folder that only the user can open, below the user's
/// runtime folder, or below the temporary folder when that is unknown.
///
/// # Errors
///
/// Returns why the folder couldn't be made or locked.
fn temporary_folder() -> Result<(PathBuf, File), String> {
    let base = hapto_host::runtime_folder().unwrap_or_else(std::env::temp_dir);
    remove_left_behind(&base);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let folder = base.join(format!("{TEMPORARY}{}-{now}", std::process::id()));
    DirBuilder::new()
        .mode(0o700)
        .create(&folder)
        .map_err(|error| format!("couldn't make {}: {error}", folder.display()))?;
    let lock = lock(&folder)?;
    Ok((folder, lock))
}

/// Removes each temporary folder in `base` whose lock no process holds, as
/// a helper that was killed leaves its folder. A folder of another user's
/// stays, because this user can't open its lock.
fn remove_left_behind(base: &Path) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let ours = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(TEMPORARY));
        if ours && let Ok(_held) = lock(&entry.path()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Starts GTK on its X11 backend.
///
/// # Errors
///
/// Returns why GTK didn't start.
fn start_gtk() -> Result<(), String> {
    gtk::gdk::set_allowed_backends("x11");
    gtk::init().map_err(|error| format!("GTK did not start: {error}"))?;
    // A host may destroy its window, and with it a page's plug, at any
    // moment, and GDK ends the process at an X error that no trap catches.
    // This trap is never popped, so it catches every X error that an inner
    // trap doesn't, and one host window can't end the pages in the others.
    gtk::gdk::error_trap_push();
    Ok(())
}

/// The helper's open pages, and what they share.
struct Pages {
    /// The open pages, which drop before the context that they share.
    pages: HashMap<PageId, OpenPage>,
    context: WebContext,
    assets: &'static [Asset],
    dev: Option<String>,
    devtools: bool,
    keyboard: Rc<Keyboard>,
    to_editor: ToEditor,
    requests: Receiver<Request<'static>>,
}

impl Pages {
    /// Runs GTK's main loop, which applies the editors' requests whenever
    /// `woken` is readable, until the editors shut the socket.
    fn run(mut self, woken: UnixStream) {
        glib::unix_fd_add_local(woken.as_raw_fd(), IOCondition::IN, move |_, _| {
            // Empties `woken`, whose bytes say only that requests arrived.
            let mut bytes = [0; 64];
            while matches!((&woken).read(&mut bytes), Ok(read) if read > 0) {}
            self.poll()
        });
        gtk::main();
    }

    /// Applies each request that the editors sent since the last call, and
    /// quits GTK's main loop once they have shut the socket.
    fn poll(&mut self) -> ControlFlow {
        loop {
            match self.requests.try_recv() {
                Ok(request) => self.apply(request),
                Err(TryRecvError::Empty) => return ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => {
                    gtk::main_quit();
                    return ControlFlow::Break;
                }
            }
        }
    }

    fn apply(&mut self, request: Request<'_>) {
        match request {
            Request::Open(id, open) => {
                let answer = match self.open(id, open) {
                    Ok(page) => {
                        self.pages.insert(id, page);
                        FromHelper::Opened(id)
                    }
                    Err(reason) => FromHelper::Failed(id, reason),
                };
                self.to_editor.send(&answer);
            }
            Request::Script(id, script) => {
                if let Some(page) = self.pages.get(&id) {
                    page.evaluate(&script);
                }
            }
            Request::Place(id, place) => {
                if let Some(page) = self.pages.get(&id) {
                    page.place(place);
                }
            }
            Request::FocusHost(id) => {
                if let Some(page) = self.pages.get(&id) {
                    self.keyboard.give_to_host(&page.plug, page.window);
                }
            }
            Request::Close(id) => {
                if let Some(page) = self.pages.remove(&id) {
                    self.keyboard.forget(page.window);
                    page.close();
                }
                self.to_editor.send(&FromHelper::Closed(id));
            }
        }
    }

    /// Opens the page `id` in a plug inside the host's window that `open`
    /// names.
    ///
    /// # Errors
    ///
    /// Returns why the page didn't open.
    fn open(&mut self, id: PageId, open: Open) -> Result<OpenPage, String> {
        // gtk-rs takes an X11 window's id as an `i32`. A server's ids fit in
        // 29 bits.
        let socket = i32::try_from(open.window)
            .map_err(|_| format!("the host's window id {} is too large", open.window))?;
        let plug = plug_in(socket).ok_or("the host's window is gone")?;
        let Some(window) = u32::try_from(plug.id()).ok().and_then(NonZeroU32::new) else {
            plug.close();
            return Err("the plug has no X11 window".to_owned());
        };
        let to_editor = self.to_editor.clone();
        let builder = wry_page::builder(
            &mut self.context,
            ORIGIN,
            self.dev.clone(),
            self.assets,
            move |message| to_editor.send(&FromHelper::Message(id, message)),
        )
        // wry's own default follows the helper's build, not the plugin's.
        .with_devtools(self.devtools);
        let builder = if self.devtools {
            builder
        } else {
            builder.with_initialization_script(NO_CONTEXT_MENU)
        };
        let webview = builder.build_gtk(&plug).map_err(|error| {
            plug.close();
            format!("the WebView did not start: {error}")
        })?;
        let click = take_keys_at_a_click(&plug, window, Rc::clone(&self.keyboard));
        show(&plug);
        let page = OpenPage {
            webview,
            plug,
            window,
            _click: click,
        };
        page.place(open.place);
        // The editor's open returns once the plug is in the host's window,
        // where the host may look for it at once.
        sync();
        Ok(page)
    }
}

/// A page, open in its host's window.
struct OpenPage {
    webview: WebView,
    plug: gtk::Plug,
    /// The plug's X11 window.
    window: NonZeroU32,
    /// Gives the page the keyboard at each click. GTK 3 doesn't keep a
    /// widget's gestures alive, so the page does.
    _click: gtk::GestureMultiPress,
}

impl OpenPage {
    fn evaluate(&self, script: &str) {
        let _ = self.webview.evaluate_script(script);
    }

    /// Sizes the plug in GDK's units, and moves its window to the top left of
    /// the host's, because a host that isn't an XEmbed embedder never sizes
    /// it. GDK scales a window only by a whole number, so the page's zoom
    /// covers the rest of the host's scale, and the page lays out in the same
    /// CSS pixels as on Windows.
    fn place(&self, place: Place) {
        let zoom = place.scale / f64::from(self.plug.scale_factor());
        let units = |logical: u32| ((f64::from(logical) * zoom).round() as i32).max(1);
        let (width, height) = (units(place.width), units(place.height));
        self.plug.resize(width, height);
        if let Some(window) = self.plug.window() {
            window.move_resize(0, 0, width, height);
        }
        let _ = self.webview.zoom(zoom);
    }

    /// Takes the page out of the host's window before it returns, which
    /// [`Request::Close`] needs. The plug's window is unmapped at once, and
    /// GTK destroys the plug in an idle callback.
    fn close(self) {
        let OpenPage { webview, plug, .. } = self;
        // Dropped, wry's WebView destroys its widget.
        drop(webview);
        plug.hide();
        plug.close();
        sync();
    }
}

/// Waits until the X server has carried out every request that GDK made.
fn sync() {
    if let Some(display) = gtk::gdk::Display::default() {
        display.sync();
    }
}

/// Returns a realized plug inside the X11 window `socket`, or `None` if that
/// window is gone, because GTK would then make the plug a window of its own on
/// the screen.
fn plug_in(socket: i32) -> Option<gtk::Plug> {
    let plug = gtk::Plug::new(socket);
    plug.realize();
    if plug
        .socket_window()
        .is_some_and(|window| is_on_server(&window))
    {
        Some(plug)
    } else {
        plug.close();
        None
    }
}

/// Shows the plug and maps its window. In XEmbed the embedder maps the
/// plug's window. A host without XEmbed never does, so the helper maps it.
fn show(plug: &gtk::Plug) {
    plug.show();
    if let Some(window) = plug.window() {
        window.show();
    }
}

/// Whether the X server still has `window`. GDK may keep a window's wrapper
/// after the server destroyed the window, so this calls `XGetGeometry` and
/// waits for the reply.
fn is_on_server(window: &gtk::gdk::Window) -> bool {
    gtk::gdk::error_trap_push();
    let _ = window.geometry();
    gtk::gdk::error_trap_pop() == 0
}

/// Returns a gesture that gives the page the keyboard whenever it is
/// clicked, and leaves the keyboard with the host until then. The gesture
/// runs in the capture phase, so it sees the press before the page takes it.
fn take_keys_at_a_click(
    plug: &gtk::Plug,
    window: NonZeroU32,
    keyboard: Rc<Keyboard>,
) -> gtk::GestureMultiPress {
    let click = gtk::GestureMultiPress::new(plug);
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let pressed = plug.downgrade();
    click.connect_pressed(move |_, _, _, _| {
        if let Some(plug) = pressed.upgrade() {
            keyboard.give_to_page(&plug, window);
        }
    });
    select_keys(plug, false);
    click
}

/// Selects key events on the plug's window, or stops selecting them. X sends
/// a key to the window under the pointer when that window is inside the
/// focused one, and passes it up through the window's parents to the first
/// that selects it, so a key over a plug that doesn't select keys reaches
/// the host's window.
fn select_keys(plug: &gtk::Plug, selected: bool) {
    if let Some(window) = plug.window() {
        let keys = EventMask::KEY_PRESS_MASK | EventMask::KEY_RELEASE_MASK;
        let events = window.events();
        window.set_events(if selected {
            events | keys
        } else {
            events - keys
        });
    }
}

/// XEmbed's messages from an embedder to its plug.
#[derive(Clone, Copy)]
#[repr(u32)]
enum XEmbed {
    WindowActivate = 1,
    WindowDeactivate = 2,
    FocusIn = 4,
    FocusOut = 5,
}

/// XEmbed's detail of a focus that stays on the widget that had it.
const XEMBED_FOCUS_CURRENT: u32 = 0;

/// The keyboard over the helper's pages: the page that holds it, from a
/// click until the hand-back, and the helper's own connection to the X
/// server, through which it tells each plug, as an XEmbed embedder would,
/// whether it is active and has the keyboard.
///
/// GTK counts a plug as active only when its embedder says so, and hosts
/// aren't XEmbed embedders, so the page would otherwise never have the focus,
/// and `document.hasFocus()` would stay false while the user types into it.
/// The page's transport hands the keyboard back only while it has the focus.
/// The helper never moves X's focus, which the host and the window manager
/// own.
struct Keyboard {
    x: RustConnection,
    xembed: xproto::Atom,
    holder: RefCell<Option<Holder>>,
}

/// The page that holds the keyboard.
struct Holder {
    window: NonZeroU32,
    plug: WeakRef<gtk::Plug>,
}

impl Keyboard {
    /// Connects to the X server that GDK uses.
    ///
    /// # Errors
    ///
    /// Returns why the connection failed.
    fn connect() -> Result<Keyboard, String> {
        fn failed(error: impl Display) -> String {
            format!("the helper couldn't connect to the X server: {error}")
        }
        let (x, _) = x11rb::connect(None).map_err(failed)?;
        let xembed = x
            .intern_atom(false, b"_XEMBED")
            .map_err(failed)?
            .reply()
            .map_err(failed)?
            .atom;
        Ok(Keyboard {
            x,
            xembed,
            holder: RefCell::new(None),
        })
    }

    /// Gives the keyboard to the page in `plug`, whose X11 window is
    /// `window`, and takes it from the page that held it.
    fn give_to_page(&self, plug: &gtk::Plug, window: NonZeroU32) {
        let holder = Holder {
            window,
            plug: plug.downgrade(),
        };
        if let Some(previous) = self.holder.replace(Some(holder))
            && previous.window != window
            && let Some(previous_plug) = previous.plug.upgrade()
        {
            self.release(&previous_plug, previous.window);
        }
        select_keys(plug, true);
        self.send(window, [XEmbed::WindowActivate, XEmbed::FocusIn]);
    }

    /// Gives the keyboard back to the host from the page in `plug`, whose X11
    /// window is `window`.
    fn give_to_host(&self, plug: &gtk::Plug, window: NonZeroU32) {
        self.forget(window);
        self.release(plug, window);
    }

    /// Forgets the page in `window` as the page that holds the keyboard, as
    /// when the page closes.
    fn forget(&self, window: NonZeroU32) {
        let mut holder = self.holder.borrow_mut();
        if holder
            .as_ref()
            .is_some_and(|holder| holder.window == window)
        {
            *holder = None;
        }
    }

    fn release(&self, plug: &gtk::Plug, window: NonZeroU32) {
        plug.set_focus(None::<&gtk::Widget>);
        select_keys(plug, false);
        self.send(window, [XEmbed::FocusOut, XEmbed::WindowDeactivate]);
    }

    /// Sends the plug in `window` each of `messages`. The X server hands a
    /// message sent with no event mask to the client that made the window,
    /// which is GDK. An error, as for a plug whose window is gone, is
    /// dropped, and so is every event that the connection gets.
    fn send(&self, window: NonZeroU32, messages: [XEmbed; 2]) {
        for message in messages {
            let data = [
                x11rb::CURRENT_TIME,
                message as u32,
                XEMBED_FOCUS_CURRENT,
                0,
                0,
            ];
            let event = ClientMessageEvent::new(32, window.get(), self.xembed, data);
            let _ = self
                .x
                .send_event(false, window.get(), xproto::EventMask::NO_EVENT, event)
                .map(VoidCookie::ignore_error);
        }
        let _ = self.x.flush();
        while let Ok(Some(_)) = self.x.poll_for_event() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::{is_page, page_url};

    #[test]
    fn the_page_built_in_is_the_page_at_webkitgtks_origin() {
        assert!(is_page(&page_url(None), ORIGIN, None));
    }

    #[test]
    fn a_second_helper_on_a_locked_folder_keeps_its_data_apart_and_removes_it() {
        let (folder, _test) = temporary_folder().expect("a folder for the test");
        drop(_test);
        let first = DataFolder::new(folder.clone()).expect("the first helper's folder");
        assert_eq!(first.path(), folder);
        let second = DataFolder::new(folder.clone()).expect("the second helper's folder");
        let apart = second.path().to_owned();
        assert_ne!(apart, folder, "the second helper's data is apart");
        assert!(apart.is_dir());
        drop(second);
        assert!(!apart.exists(), "its folder goes as it exits");
        drop(first);
        let again = DataFolder::new(folder.clone()).expect("a later helper's folder");
        assert_eq!(again.path(), folder, "the lock goes with the first helper");
        drop(again);
        std::fs::remove_dir_all(folder).expect("the test's folder goes");
    }

    #[test]
    fn a_temporary_folder_that_no_helper_holds_is_removed() {
        let (base, _test) = temporary_folder().expect("a folder for the test");
        let left = base.join(format!("{TEMPORARY}1-1"));
        let held = base.join(format!("{TEMPORARY}2-2"));
        for folder in [&left, &held] {
            std::fs::create_dir_all(folder.join("storage")).expect("a folder");
        }
        let _held = lock(&held).expect("a helper that still runs");
        remove_left_behind(&base);
        assert!(!left.exists(), "a folder that no helper holds goes");
        assert!(held.exists(), "a running helper's folder stays");
        drop(_held);
        std::fs::remove_dir_all(base).expect("the test's folder goes");
    }
}
