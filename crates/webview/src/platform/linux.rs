//! WebKitGTK, the web engine on Linux, in a helper process.
//!
//! Each plugin has one helper, `hapto-webview-helper`, in each host process,
//! and that helper shows all of the plugin's open pages, because WebKitGTK
//! expects only one process to use a data folder. WebView2 on Windows shares
//! one browser process for a folder in the same way.
//!
//! - The first editor to open starts the helper.
//! - The editors send it [`wire`]'s frames over a Unix socket, which is the
//!   helper's standard input.
//! - The helper exits after the last page closes.
//!
//! The host's process loads no GTK, so a host that uses GDK itself, GTK 2 or
//! GTK 4 keeps working, and an X error in GTK can't end the host.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{OsStr, c_ulong};
use std::io;
use std::net::Shutdown;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::{Rc, Weak};
use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use hapto_host::{ParentWindow, Size};
use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};

use super::{Load, WebEngine};
use crate::wire::{self, Frame, FromHelper, HELPER, Open, PageId, Place, Request, Setup, Start};

/// A page in WebKitGTK, which its plugin's helper shows.
pub(crate) struct WebKitGtk {
    helper: Rc<Helper>,
    id: PageId,
    receive: Box<dyn Fn(String)>,
    /// Whether `poll` has said that the page is gone.
    gone: Cell<bool>,
}

/// The name of the thread that reads what a helper sends.
const READER: &str = "hapto-webview-reader";

/// How long the editor waits for the helper to open a page, and for a write
/// to the helper to finish.
const TIMEOUT: Duration = Duration::from_secs(10);

/// How long the editor waits for the helper to take a page out of the host's
/// window, and to exit once its last page has closed, before it kills the
/// helper.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

/// The environment variables that the helper starts without. GDK tries only
/// the backends that `GDK_BACKEND` names, so a host's choice of Wayland would
/// keep the helper's X11 backend from opening. The rest point a process at
/// other copies of its libraries and modules, as Ardour's launcher points
/// Ardour at the copies in its bundle, and the system's GTK and WebKitGTK in
/// the helper can't load those.
const HOST_ONLY: &[&str] = &[
    "GDK_BACKEND",
    "LD_LIBRARY_PATH",
    "LD_PRELOAD",
    "GTK_PATH",
    "GTK_MODULES",
    "GTK_IM_MODULE_FILE",
    "GDK_PIXBUF_MODULE_FILE",
    "GDK_PIXBUF_MODULEDIR",
    "GIO_MODULE_DIR",
    "GIO_EXTRA_MODULES",
    "FONTCONFIG_FILE",
    "FONTCONFIG_PATH",
];

/// The helper that [`set_helper`] named.
static NAMED: OnceLock<PathBuf> = OnceLock::new();

thread_local! {
    /// The running helpers of this thread's editors, by the data folder that
    /// each uses.
    static HELPERS: RefCell<HashMap<PathBuf, Weak<Helper>>> = RefCell::default();
}

/// Names the page's helper program, for an app or a test that keeps it
/// elsewhere than beside its module. The first call takes effect, and later
/// ones change nothing.
#[doc(hidden)]
pub fn set_helper(program: PathBuf) {
    let _ = NAMED.set(program);
}

impl WebEngine for WebKitGtk {
    const FOLDER: &'static str = "WebKitGTK";

    fn open(
        load: Load,
        parent: &ParentWindow,
        size: Size,
        scale: f64,
    ) -> Result<WebKitGtk, String> {
        let open = Open {
            window: x11_window(parent)?,
            place: Place::new(size, scale),
        };
        let deadline = Instant::now() + TIMEOUT;
        let (helper, id) = match Helper::running(&load.folder) {
            Some(helper) => match helper.open_page(open, deadline) {
                // A helper that has just exited looks alive until its reader
                // sees the end of the socket, so the open tries once more
                // with a new helper then.
                Err(_) if helper.connection.ended() => {
                    Helper::start(&load, deadline)?.open_in(open, deadline)?
                }
                opened => (helper, opened?),
            },
            None => Helper::start(&load, deadline)?.open_in(open, deadline)?,
        };
        Ok(WebKitGtk {
            helper,
            id,
            receive: load.receive,
            gone: Cell::new(false),
        })
    }

    /// Returns the user's runtime folder, which only the user can open.
    fn fallback() -> Option<PathBuf> {
        hapto_host::runtime_folder()
    }

    fn evaluate(&self, script: &str) {
        let _ = self.send(&Request::Script(self.id, Cow::Borrowed(script)));
    }

    fn place(&self, size: Size, scale: f64) {
        let _ = self.send(&Request::Place(self.id, Place::new(size, scale)));
    }

    fn focus_host(&self) -> Result<(), String> {
        self.send(&Request::FocusHost(self.id))
    }

    /// Passes on the messages that had arrived from the page when `poll`
    /// started. Messages that arrive while it runs wait for the next tick, so
    /// a page that posts without pause can't hold the host's thread.
    fn poll(&self) -> Option<String> {
        let connection = &self.helper.connection;
        connection.receive();
        for message in connection.take_inbox(self.id) {
            (self.receive)(message);
        }
        if connection.ended() && !self.gone.replace(true) {
            return Some("the connection to the page's helper ended".to_owned());
        }
        None
    }
}

impl WebKitGtk {
    fn send(&self, request: &Request<'_>) -> Result<(), String> {
        self.helper
            .connection
            .send(request, Instant::now() + TIMEOUT)
    }
}

impl Drop for WebKitGtk {
    /// Closes the page, and passes on the messages that it posted before it
    /// closed, as WebView2 does on Windows.
    fn drop(&mut self) {
        for message in self.helper.connection.close_page(self.id) {
            (self.receive)(message);
        }
    }
}

/// A helper process, which shows every open page of one plugin in the host's
/// process.
///
/// Thread: main.
struct Helper {
    child: Child,
    connection: Connection,
}

impl Helper {
    /// Returns the running helper that uses `folder`, unless it has gone.
    fn running(folder: &Path) -> Option<Rc<Helper>> {
        let helper = HELPERS.with_borrow(|helpers| helpers.get(folder).and_then(Weak::upgrade))?;
        helper.connection.receive();
        (!helper.connection.ended()).then_some(helper)
    }

    /// Starts a helper for the page that `load` describes, and sends it what
    /// its pages share, then the page's files.
    ///
    /// # Errors
    ///
    /// Returns why the helper didn't start or take them.
    fn start(load: &Load, deadline: Instant) -> Result<Rc<Helper>, String> {
        let (connection, helper_end) = Connection::new()
            .map_err(|error| format!("couldn't make a socket for the page's helper: {error}"))?;
        let child = command(&program()?, load.accelerated)
            .stdin(OwnedFd::from(helper_end))
            .spawn()
            .map_err(|error| format!("the page's helper didn't start: {error}"))?;
        // If a step below fails, dropping `helper` stops it.
        let helper = Helper { child, connection };
        let start = Start {
            files: load.assets.len(),
            folder: load.folder.as_os_str().as_bytes().to_vec(),
            dev: load.dev.clone(),
            devtools: load.devtools,
        };
        let sent = helper
            .connection
            .send(&Setup::Start(start), deadline)
            .and_then(|()| {
                load.assets.iter().try_for_each(|asset| {
                    let file = Setup::File {
                        path: Cow::Borrowed(asset.path),
                        bytes: Cow::Borrowed(asset.bytes),
                    };
                    helper.connection.send(&file, deadline)
                })
            });
        if let Err(error) = sent {
            return Err(helper.connection.stopped().unwrap_or(error));
        }
        let helper = Rc::new(helper);
        HELPERS.with_borrow_mut(|helpers| {
            helpers.retain(|_, helper| helper.strong_count() > 0);
            helpers.insert(load.folder.clone(), Rc::downgrade(&helper));
        });
        Ok(helper)
    }

    /// Opens a page where `open` says, and returns this helper with the
    /// page's id.
    ///
    /// # Errors
    ///
    /// Returns why the page didn't open.
    fn open_in(
        self: Rc<Helper>,
        open: Open,
        deadline: Instant,
    ) -> Result<(Rc<Helper>, PageId), String> {
        let id = self.open_page(open, deadline)?;
        Ok((self, id))
    }

    /// Opens a page where `open` says, and returns its id.
    ///
    /// # Errors
    ///
    /// Returns why the page didn't open. The page is closed then.
    fn open_page(&self, open: Open, deadline: Instant) -> Result<PageId, String> {
        let id = self.connection.add_page();
        let opened = self
            .connection
            .ask(&Request::Open(id, open), Answer::Opened, deadline);
        if opened.is_err() {
            self.connection.close_page(id);
        }
        opened.map(|()| id)
    }
}

impl Drop for Helper {
    /// Shuts the socket's write half, which makes the helper quit, and waits
    /// up to [`CLOSE_TIMEOUT`] for it to exit. It kills a helper that is still
    /// running then, so no helper outlives its pages. The connection drops
    /// after this, so it reads what the helper sends until the helper exits.
    fn drop(&mut self) {
        self.connection.finish();
        if !exits_within(&mut self.child, CLOSE_TIMEOUT) {
            let _ = self.child.kill();
            // A process that is stuck in the kernel exits only once the call
            // that it is in returns. The editor waits no longer than this for
            // it, so a close can't hang the host, and such a helper stays a
            // zombie until the host exits.
            let _ = exits_within(&mut self.child, CLOSE_TIMEOUT);
        }
    }
}

/// Waits up to `timeout` for `child` to exit, and returns whether it did.
fn exits_within(child: &mut Child, timeout: Duration) -> bool {
    let end = Instant::now() + timeout;
    loop {
        // An error means that the host reaped the helper itself, as a host
        // that ignores `SIGCHLD` does.
        if !matches!(child.try_wait(), Ok(None)) {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// The answer to an open or a close that the editor waits for.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Answer {
    Opened,
    Closed,
}

/// The editors' end of the socket to a helper, the thread that reads it, and
/// each open page's inbox.
///
/// std writes to a Unix socket with `MSG_NOSIGNAL`, so a write to a helper
/// that has exited returns an error, where a write to a pipe would raise
/// `SIGPIPE`, which ends a host that doesn't ignore it. std's vectored writes
/// don't pass that flag, so a frame must go out through `write_all`.
struct Connection {
    socket: UnixStream,
    /// Every frame that the helper sent, as the thread read it.
    from_helper: Receiver<FromHelper>,
    reader: Option<JoinHandle<io::Result<()>>>,
    /// Whether the connection has ended, so the helper has exited or is
    /// about to.
    ended: Cell<bool>,
    /// The messages that each open page posted and its editor hasn't passed
    /// on yet.
    inboxes: RefCell<HashMap<PageId, Vec<String>>>,
    /// The id of the next page to open.
    next: Cell<u32>,
}

impl Connection {
    /// Returns a connection and the helper's end of its socket.
    fn new() -> io::Result<(Connection, UnixStream)> {
        let (socket, helper_end) = UnixStream::pair()?;
        // The editor takes what arrived at each tick, so the thread wakes
        // nothing.
        let (from_helper, reader) = wire::read_on_a_thread(READER, socket.try_clone()?, || ())?;
        let connection = Connection {
            socket,
            from_helper,
            reader: Some(reader),
            ended: Cell::new(false),
            inboxes: RefCell::default(),
            next: Cell::new(0),
        };
        Ok((connection, helper_end))
    }

    /// Sends `frame`, and fails once `deadline` passes, so a helper that
    /// stopped reading can't hang the host's thread. After a failed write the
    /// helper can't tell where the next frame starts, so the connection ends.
    fn send(&self, frame: &impl Frame, deadline: Instant) -> Result<(), String> {
        // `set_write_timeout` refuses a zero duration. The timeout applies to
        // each write call rather than to the whole frame.
        let left = deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1));
        self.socket
            .set_write_timeout(Some(left))
            .and_then(|()| frame.write(&mut &self.socket))
            .map_err(|error| {
                self.end();
                format!("couldn't write to the page's helper: {error}")
            })
    }

    /// Sends `request`, and waits until `deadline` for `answer` about its
    /// page.
    ///
    /// # Errors
    ///
    /// Returns why the request failed, or why the helper didn't answer.
    fn ask(&self, request: &Request<'_>, answer: Answer, deadline: Instant) -> Result<(), String> {
        self.send(request, deadline)?;
        self.answer(request.page(), answer, deadline)
    }

    /// Waits until `deadline` for `answer` about the page `id`, and puts the
    /// messages that arrive meanwhile in their pages' inboxes.
    ///
    /// # Errors
    ///
    /// Returns why the page didn't open, or why the helper didn't answer.
    fn answer(&self, id: PageId, answer: Answer, deadline: Instant) -> Result<(), String> {
        let late = || "the page's helper didn't answer in time".to_owned();
        loop {
            let left = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(late)?;
            match self.from_helper.recv_timeout(left) {
                Ok(FromHelper::Opened(page)) if (page, answer) == (id, Answer::Opened) => {
                    return Ok(());
                }
                Ok(FromHelper::Closed(page)) if (page, answer) == (id, Answer::Closed) => {
                    return Ok(());
                }
                Ok(FromHelper::Failed(page, reason)) if page == id => return Err(reason),
                Ok(FromHelper::Stopped(reason)) => {
                    self.end();
                    return Err(reason);
                }
                Ok(frame) => self.route(frame),
                Err(RecvTimeoutError::Timeout) => return Err(late()),
                Err(RecvTimeoutError::Disconnected) => {
                    self.end();
                    return Err("the page's helper exited".to_owned());
                }
            }
        }
    }

    /// Puts each message that has arrived from the helper in its page's
    /// inbox.
    fn receive(&self) {
        loop {
            match self.from_helper.try_recv() {
                Ok(frame) => self.route(frame),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.end();
                    return;
                }
            }
        }
    }

    /// Puts a message in its page's inbox. A reply to an open or a close is
    /// dropped here, because only `answer` waits for one.
    fn route(&self, frame: FromHelper) {
        match frame {
            FromHelper::Message(page, message) => {
                if let Some(inbox) = self.inboxes.borrow_mut().get_mut(&page) {
                    inbox.push(message);
                }
            }
            FromHelper::Stopped(_) => self.end(),
            FromHelper::Opened(_) | FromHelper::Failed(..) | FromHelper::Closed(_) => {}
        }
    }

    /// Returns a new page's id, and makes its inbox.
    fn add_page(&self) -> PageId {
        let id = PageId(self.next.get());
        self.next.set(id.0.wrapping_add(1));
        self.inboxes.borrow_mut().insert(id, Vec::new());
        id
    }

    /// Returns the messages in the page `id`'s inbox, and empties it.
    fn take_inbox(&self, id: PageId) -> Vec<String> {
        self.inboxes
            .borrow_mut()
            .get_mut(&id)
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// Closes the page `id`, waiting up to [`CLOSE_TIMEOUT`] for the helper's
    /// answer, and returns the messages that the page posted before it
    /// closed.
    fn close_page(&self, id: PageId) -> Vec<String> {
        let deadline = Instant::now() + CLOSE_TIMEOUT;
        let _ = self.ask(&Request::Close(id), Answer::Closed, deadline);
        self.inboxes.borrow_mut().remove(&id).unwrap_or_default()
    }

    /// Returns why the helper stopped, when it says so in the next moment,
    /// as a helper that refused its start does before it exits.
    fn stopped(&self) -> Option<String> {
        match self.from_helper.recv_timeout(CLOSE_TIMEOUT) {
            Ok(FromHelper::Stopped(reason)) => Some(reason),
            _ => None,
        }
    }

    /// Tells the helper that its editors send nothing more.
    fn finish(&self) {
        let _ = self.socket.shutdown(Shutdown::Write);
    }

    /// Ends the connection. The helper reads the end of the socket and exits,
    /// and the reader goes on until it does.
    fn end(&self) {
        self.finish();
        self.ended.set(true);
    }

    fn ended(&self) -> bool {
        self.ended.get()
    }
}

impl Drop for Connection {
    /// Shuts the socket down and joins the reader, so no thread runs the
    /// plugin's code once the last editor has closed. The shutdown ends the
    /// reader's wait even while another process still holds the helper's end.
    fn drop(&mut self) {
        let _ = self.socket.shutdown(Shutdown::Both);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// Returns the X11 window that `parent` names.
///
/// # Errors
///
/// Returns why `parent` isn't an X11 window.
fn x11_window(parent: &ParentWindow) -> Result<c_ulong, String> {
    let handle = parent.window_handle().map_err(|error| error.to_string())?;
    match handle.as_raw() {
        RawWindowHandle::Xlib(host) => Ok(host.window),
        _ => Err("the host's window isn't an X11 window".to_owned()),
    }
}

/// Returns the command that starts the helper `program`, with WebKitGTK
/// drawing with the GPU only when `accelerated` says so.
fn command(program: &Path, accelerated: bool) -> Command {
    let mut command = Command::new(program);
    command
        // Nothing reads what the helper writes to its standard output.
        .stdout(Stdio::null())
        // A group of its own, so a Ctrl+C meant for the host's terminal
        // doesn't end the pages while the host goes on.
        .process_group(0);
    for variable in HOST_ONLY {
        command.env_remove(variable);
    }
    if !accelerated {
        // For why, see `Page::with_hardware_acceleration`. The variable
        // reaches WebKitGTK's own processes as well. WebKitGTK's hardware
        // acceleration policy can be set only once wry has built the view,
        // and a page set to never that way stopped responding in WebKitGTK
        // 2.52.
        command.env("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
    command
}

/// Returns the helper's program: the one that [`set_helper`] named, or the
/// file beside the plugin's module.
///
/// # Errors
///
/// Returns why there is no helper to start.
fn program() -> Result<PathBuf, String> {
    if let Some(named) = NAMED.get() {
        return Ok(named.clone());
    }
    let module = module().ok_or("the plugin's module isn't in /proc/self/maps")?;
    let program = module.with_file_name(HELPER);
    if program.is_file() {
        Ok(program)
    } else {
        Err(format!(
            "the page's helper isn't installed at {}",
            program.display()
        ))
    }
}

/// Returns the file of the module that this code is in, the plugin's or the
/// app's, from the mappings in `/proc/self/maps`.
fn module() -> Option<PathBuf> {
    let address = module as fn() -> Option<PathBuf> as usize;
    let maps = std::fs::read("/proc/self/maps").ok()?;
    maps.split(|&byte| byte == b'\n').find_map(|line| {
        let range = line.split(|&byte| byte == b' ').next()?;
        let (start, end) = std::str::from_utf8(range).ok()?.split_once('-')?;
        let start = usize::from_str_radix(start, 16).ok()?;
        let end = usize::from_str_radix(end, 16).ok()?;
        if !(start..end).contains(&address) {
            return None;
        }
        // A mapping's file is the line's last field, the one that starts
        // with a slash. Its bytes need not be UTF-8.
        let at = line.iter().position(|&byte| byte == b'/')?;
        Some(PathBuf::from(OsStr::from_bytes(line.get(at..)?)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webkitgtk_s_folder_keeps_its_name() {
        // A new name would leave every user's WebKitGTK data behind.
        assert_eq!(WebKitGtk::FOLDER, "WebKitGTK");
    }

    #[test]
    fn this_code_s_module_is_the_running_test() {
        let module = module().expect("a mapping that holds this code");
        let running = std::env::current_exe().expect("the test's own file");
        assert_eq!(module.file_name(), running.file_name());
    }

    #[test]
    fn a_frame_to_a_helper_that_has_gone_is_an_error_and_ends_the_connection() {
        let (connection, helper_end) = Connection::new().expect("a socket");
        drop(helper_end);
        let deadline = Instant::now() + TIMEOUT;
        let request = Request::FocusHost(PageId(0));
        assert!(connection.send(&request, deadline).is_err());
        assert!(connection.ended());
        assert!(connection.send(&request, deadline).is_err());
        assert!(connection.from_helper.recv().is_err());
    }

    #[test]
    fn a_close_waits_for_its_own_answer_and_routes_the_rest_to_their_pages() {
        let (connection, helper_end) = Connection::new().expect("a socket");
        let (closing, open) = (connection.add_page(), connection.add_page());
        for frame in [
            FromHelper::Message(closing, String::from("last")),
            FromHelper::Opened(closing),
            FromHelper::Message(open, String::from("other")),
            FromHelper::Closed(closing),
        ] {
            frame.write(&mut &helper_end).expect("a frame");
        }
        let deadline = Instant::now() + TIMEOUT;
        assert_eq!(
            connection.answer(closing, Answer::Closed, deadline),
            Ok(()),
            "a late Opened doesn't end the wait for Closed"
        );
        assert_eq!(connection.take_inbox(open), ["other"]);
        assert_eq!(connection.take_inbox(closing), ["last"]);
    }

    #[test]
    fn the_helper_starts_without_the_host_s_library_paths_and_draws_in_software_by_default() {
        let envs = |accelerated| {
            command(Path::new("helper"), accelerated)
                .get_envs()
                .map(|(name, value)| (name.to_owned(), value.map(OsStr::to_owned)))
                .collect::<HashMap<_, _>>()
        };
        let software = envs(false);
        for variable in HOST_ONLY {
            assert_eq!(
                software.get(OsStr::new(variable)),
                Some(&None),
                "{variable}"
            );
        }
        let renderer = OsStr::new("WEBKIT_DISABLE_DMABUF_RENDERER");
        assert_eq!(
            software.get(renderer),
            Some(&Some(OsStr::new("1").to_owned()))
        );
        assert_eq!(envs(true).get(renderer), None);
    }
}
