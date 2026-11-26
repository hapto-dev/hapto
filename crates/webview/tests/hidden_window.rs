//! The page in a real web engine, in a window of the test's own that is never
//! shown. On Windows that is WebView2, through the VST3 exporter as a host
//! drives it, and on Linux WebKitGTK, through the standalone exporter. The
//! page loads from the files built into the plugin, its `ready` is answered
//! with a snapshot, its `set` reaches the parameter and its `setValue` the
//! value, a native change to the value reaches it at the tick, and it cannot
//! navigate away.
//!
//! Ignored in a normal run, because it needs the web engine and starts its
//! processes. Run it by hand with `cargo test -p hapto-webview --features
//! helper --test hidden_window -- --ignored`, on Linux in an X server such as
//! Xvfb's. The feature builds the page's helper on Linux and changes nothing
//! on Windows.
//!
//! The page's script drives each test. They take no focus, show no window
//! and send no input, except on Linux, where two tests map their window to
//! read the page's pixels back and to send keys and a click through XTest.

#![cfg(hapto_wry)]

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use exporter::{Host, expect_faults, level, turn, view};
use hapto_host::layout::{AnyLayout, Width};
use hapto_host::vst3::HiddenWindow;
use hapto_host::{
    AuCodes, Block, Category, Editor, Parts, Plugin, PluginInfo, ProcessSetup, Processor, Size,
    Status,
};
use hapto_params::{Declarer, Float, FloatDecl, Params, Value};
use hapto_webview::{Asset, Page};

/// A page that speaks the protocol by hand, as `@hapto/page`'s client does:
/// it listens, sends `ready`, and once the snapshot arrives reports the
/// level and the label in it through a command, sets the level to 0.75 and
/// the label to "Page". Then it tries to leave for another address and,
/// still there a moment later, reports that it stayed. It reports each
/// label the native side sends it after the snapshot. After a click it
/// reports whether it has the focus, and a click on its right part gives the
/// keyboard back, as `@hapto/page`'s transport does after a click on a control
/// that needs no keys.
const PAGE: &[Asset] = &[Asset {
    path: "index.html",
    bytes: br#"<!doctype html>
<html><body><p>page</p><script>
  const send = (message) => window.ipc.postMessage(JSON.stringify(message));
  let call = 0;
  const seen = (text) => send({ type: "invoke", call: ++call, command: "seen", args: text });
  window.__hapto = {
    receive(text) {
      const message = JSON.parse(text);
      if (message.type === "snapshot") {
        seen(message.params[0].text);
        seen(`label ${message.values[0].value}`);
        send({ type: "set", param: "level", normalized: 0.75 });
        send({ type: "setValue", key: "label", value: "Page" });
        location.href = "http://127.0.0.1:9/";
        setTimeout(() => seen("stayed"), 300);
      }
      if (message.type === "value") seen(`label ${message.state.value}`);
    },
  };
  const focus = (when) => setTimeout(() => seen(`${when}: focus ${document.hasFocus()}`), 200);
  addEventListener("mousedown", (event) => event.clientX <= 300 && focus("clicked"), true);
  addEventListener("mouseup", (event) => {
    if (event.clientX > 300) {
      window.ipc.postMessage('{"shell":"releaseKeys"}');
      focus("handed back");
    }
  });
  send({ type: "ready", protocol: 3 });
</script></body></html>"#,
}];

thread_local! {
    /// What the page reported it saw.
    static SEEN: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// The label of the last probe made, as the test writes it natively.
    static LABEL: RefCell<Option<Value<String>>> = const { RefCell::new(None) };
}

struct ProbeParams {
    #[expect(
        dead_code,
        reason = "declared for the host and the page; read through the host"
    )]
    level: Float,
}

impl Params for ProbeParams {
    fn declare(p: &mut Declarer) -> Self {
        ProbeParams {
            level: p.add(FloatDecl::new("level", "Level", 0.0..=100.0, 50.0).with_unit("%")),
        }
    }
}

struct Through;

impl Processor for Through {
    type Layout = AnyLayout;

    fn process(&mut self, _block: Block<'_, AnyLayout>) -> Status {
        Status::Continue
    }
}

struct Probe {
    parts: Parts,
    label: Value<String>,
}

impl Plugin for Probe {
    const INFO: PluginInfo = PluginInfo {
        name: "Hapto WebView Probe",
        vendor: "Hapto",
        url: "",
        email: "",
        description: "",
        version: "0.1.0",
        id: "dev.hapto.webview-probe",
        vst3_class_id: [9; 16],
        au_codes: AuCodes {
            subtype: *b"wvpr",
            manufacturer: *b"Hapt",
        },
        category: Category::Effect,
        features: &[],
        layouts: &[Width::STEREO],
        midi_input: false,
        midi_output: false,
    };
    type Params = ProbeParams;
    type Processor = Through;

    fn new(_params: ProbeParams, parts: Parts) -> Self {
        let label = Value::new(String::from("Init"), parts.set());
        LABEL.with(|shared| *shared.borrow_mut() = Some(label.clone()));
        Probe { parts, label }
    }

    fn activate(&mut self, _setup: &ProcessSetup) -> Through {
        Through
    }

    fn editor(&mut self) -> Option<Box<dyn Editor>> {
        Page::new(&Self::INFO, self.parts.ui().clone(), PAGE)
            .with_size(Size {
                width: 400,
                height: 300,
            })
            .value("label", &self.label)
            .command("seen", |text: String| {
                SEEN.with(|seen| seen.borrow_mut().push(text));
                Ok(())
            })
            .into_editor()
    }
}

/// A plugin whose page cannot start: a file stands where its data folder
/// would be, so the editor can't make the engine's folder in it.
struct Unstartable {
    parts: Parts,
}

impl Plugin for Unstartable {
    const INFO: PluginInfo = PluginInfo {
        name: "Hapto WebView Probe That Cannot Start",
        id: "dev.hapto.webview-probe-unstartable",
        vst3_class_id: [10; 16],
        au_codes: AuCodes {
            subtype: *b"wvpu",
            manufacturer: *b"Hapt",
        },
        ..Probe::INFO
    };
    type Params = ProbeParams;
    type Processor = Through;

    fn new(_params: ProbeParams, parts: Parts) -> Self {
        Unstartable { parts }
    }

    fn activate(&mut self, _setup: &ProcessSetup) -> Through {
        Through
    }

    fn editor(&mut self) -> Option<Box<dyn Editor>> {
        Page::new(&Self::INFO, self.parts.ui().clone(), PAGE).into_editor()
    }
}

/// A file where a plugin's WebView data folder would be made, removed
/// when dropped.
struct Blocker(PathBuf);

impl Blocker {
    fn new(id: &str) -> Self {
        let path = hapto_host::data_folder(id).expect("the user's data folder");
        // On a new account, such as a CI runner's, the folder that holds the
        // plugins' data folders may not exist yet.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("the folder that holds the data folders");
        }
        std::fs::write(&path, b"").expect("a file where the data folder would be");
        Blocker(path)
    }
}

impl Drop for Blocker {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The editor through the VST3 exporter, as a host drives it.
#[cfg(windows)]
mod exporter {
    use hapto_host::Plugin;
    use hapto_host::vst3::{HiddenWindow, TestHost, TestView};

    pub(crate) type Host<P> = TestHost<P>;

    /// The plugin's view, as the host asked for it.
    pub(crate) struct View(TestView);

    /// Returns the view of `host`'s plugin.
    pub(crate) fn view<P: Plugin>(host: &Host<P>) -> View {
        View(host.view().expect("an editor"))
    }

    impl View {
        /// Attaches the view to `window`.
        pub(crate) fn open(&self, window: &HiddenWindow) -> Result<(), String> {
            self.0
                .attach(window)
                .map_err(|refused| format!("{refused:?}"))
        }

        /// Removes the view from its window.
        pub(crate) fn close(&self) {
            assert_eq!(self.0.remove(), Ok(()));
        }
    }

    /// Runs this thread's message loop for about 10 ms, as a host's would,
    /// because `pump_until`'s check returns true at its second call.
    pub(crate) fn turn<P: Plugin>(host: &Host<P>) {
        let mut asked = false;
        host.pump_until(|| std::mem::replace(&mut asked, true));
    }

    /// Says that `count` faults of `host`'s plugin are expected.
    pub(crate) fn expect_faults<P: Plugin>(host: &Host<P>, count: u64) {
        host.expect_faults(count);
    }

    /// Returns the plugin's level, as the host reads it.
    pub(crate) fn level<P: Plugin>(host: &Host<P>) -> f64 {
        let id = hapto_params::host_id("level");
        host.plain(id, host.normalized(id))
    }
}

/// The editor through the standalone exporter.
///
/// Open: the VST3 exporter refuses Linux until its X11 view is built, and
/// then these tests can drive the editor through it, as on Windows.
#[cfg(target_os = "linux")]
mod exporter {
    use std::time::Duration;

    use hapto_host::Plugin;
    use hapto_host::standalone::{Standalone, StandaloneEditor};
    use hapto_host::vst3::HiddenWindow;

    pub(crate) type Host<P> = Standalone<P>;

    /// The plugin's editor, as the app asked for it.
    pub(crate) struct View<P: Plugin>(StandaloneEditor<P>);

    /// Returns the editor of `host`'s plugin, whose page opens in the helper
    /// that Cargo builds for this test with `--features helper`.
    pub(crate) fn view<P: Plugin>(host: &Host<P>) -> View<P> {
        hapto_webview::set_helper(env!("CARGO_BIN_EXE_hapto-webview-helper").into());
        View(host.editor().expect("an editor"))
    }

    impl<P: Plugin> View<P> {
        /// Opens the editor in `window`.
        pub(crate) fn open(&self, window: &HiddenWindow) -> Result<(), String> {
            self.0.open(window).map_err(|error| error.to_string())
        }

        /// Closes the editor.
        pub(crate) fn close(&self) {
            self.0.close();
        }
    }

    /// Waits about 10 ms. GTK runs in the helper's process, and the editor
    /// reads the helper's frames at each tick, so this thread runs no message
    /// loop.
    pub(crate) fn turn<P: Plugin>(_: &Host<P>) {
        std::thread::sleep(Duration::from_millis(10));
    }

    /// Checks that `host`'s plugin has had `count` faults.
    pub(crate) fn expect_faults<P: Plugin>(host: &Host<P>, count: u64) {
        assert_eq!(host.faults(), count);
    }

    /// Returns the plugin's level, as the app reads it.
    pub(crate) fn level<P: Plugin>(host: &Host<P>) -> f64 {
        let params = host.params();
        let id = params.id("level").expect("the level");
        f64::from(params.value(id))
    }
}

/// Ticks `hosts` and runs one turn of this thread's loop until `done` holds,
/// for at most `patience`, and returns whether it holds.
fn until(hosts: &[Host<Probe>], patience: Duration, mut done: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + patience;
    while Instant::now() < end {
        for host in hosts {
            host.tick();
        }
        if done() {
            return true;
        }
        turn(&hosts[0]);
    }
    done()
}

/// Ticks `hosts` and runs this thread's loop for `time`.
fn tick_for(hosts: &[Host<Probe>], time: Duration) {
    until(hosts, time, || false);
}

/// Keeps the editors open, ticking, for 20 seconds when `HAPTO_HOLD_OPEN`
/// is set: time to read their processes and memory from outside.
fn hold_open(hosts: &[Host<Probe>]) {
    if std::env::var_os("HAPTO_HOLD_OPEN").is_some() {
        tick_for(hosts, Duration::from_secs(20));
    }
}

/// What the pages reported, in order.
fn seen() -> Vec<String> {
    SEEN.with(|seen| seen.borrow().clone())
}

/// Whether the pages reported `text`.
fn has_seen(text: &str) -> bool {
    SEEN.with(|seen| seen.borrow().iter().any(|seen| seen == text))
}

/// The label of the last probe made.
fn label() -> Value<String> {
    LABEL.with(|label| label.borrow().clone().expect("a probe was made"))
}

#[test]
#[ignore = "opens a real web engine in a hidden window: run it by hand"]
fn a_page_in_a_hidden_window_is_answered_and_its_set_reaches_the_parameter() {
    let host = Host::<Probe>::new();
    let window = HiddenWindow::new().expect("a window");
    let view = view(&host);
    let started = Instant::now();
    assert_eq!(view.open(&window), Ok(()));
    let opened = started.elapsed();
    assert_eq!(
        window.shown_children(),
        1,
        "the WebView shows once it started"
    );
    let hosts = [host];
    let answered = until(&hosts, Duration::from_secs(30), || {
        level(&hosts[0]) == 75.0 && has_seen("label Page") && has_seen("stayed")
    });
    println!(
        "open returned after {} ms; the page's set arrived after {} ms",
        opened.as_millis(),
        started.elapsed().as_millis()
    );
    assert!(
        answered,
        "the page's set, its value's answer or its stay never arrived"
    );
    hold_open(&hosts);
    // The snapshot held the label, the page's write reached it and was
    // answered, and the page's navigation was cancelled, so it stayed to
    // say so.
    assert_eq!(seen()[..2], ["50.00 %", "label Init"]);
    assert_eq!(label().get(), "Page");
    // The plugin renames itself: the next tick tells the page.
    label().set(String::from("Native"));
    let heard = until(&hosts, Duration::from_secs(10), || has_seen("label Native"));
    assert!(heard, "the page never heard the plugin's rename");
    let first = seen();
    assert_eq!(first.len(), 5, "{first:?}");
    view.close();
    // Closed: ticks reach no page.
    label().set(String::from("Closed"));
    tick_for(&hosts, Duration::from_millis(200));
    assert_eq!(seen().len(), 5);
    // Opened again: a new page asks for its snapshot and reports it.
    assert_eq!(view.open(&window), Ok(()));
    let reopened = until(&hosts, Duration::from_secs(30), || {
        seen().len() == 9 && seen().iter().filter(|seen| *seen == "stayed").count() == 2
    });
    assert!(reopened, "the reopened page never reported: {:?}", seen());
    assert_eq!(seen()[5..7], ["75.00 %", "label Closed"]);
    view.close();
    assert_eq!(window.shown_children(), 0, "closed, it left nothing");
    drop(view);
}

#[test]
#[ignore = "opens a real web engine's editor in a hidden window: run it by hand"]
fn a_page_whose_data_folder_cant_be_made_fails_to_open_and_shows_nothing() {
    let _blocker = Blocker::new(Unstartable::INFO.id);
    let host = Host::<Unstartable>::new();
    let window = HiddenWindow::new().expect("a window");
    let view = view(&host);
    assert!(
        view.open(&window).is_err(),
        "the WebView has no data folder"
    );
    expect_faults(&host, 1);
    assert_eq!(window.shown_children(), 0);
    drop(view);
}

#[test]
#[ignore = "opens sixteen real web engines in hidden windows: run it by hand"]
fn sixteen_editors_open_in_one_process() {
    let hosts: Vec<Host<Probe>> = (0..16).map(|_| Host::<Probe>::new()).collect();
    let windows: Vec<HiddenWindow> = (0..16)
        .map(|_| HiddenWindow::new().expect("a window"))
        .collect();
    let views: Vec<_> = hosts.iter().map(view).collect();
    let started = Instant::now();
    for (view, window) in views.iter().zip(&windows) {
        assert_eq!(view.open(window), Ok(()));
    }
    let opened = started.elapsed();
    let answered = until(&hosts, Duration::from_secs(60), || {
        hosts.iter().all(|host| level(host) == 75.0)
    });
    println!(
        "sixteen opens returned after {} ms; every page's set arrived after {} ms",
        opened.as_millis(),
        started.elapsed().as_millis()
    );
    assert!(answered, "not every page's set reached its parameter");
    hold_open(&hosts);
    for view in &views {
        view.close();
    }
}

/// The window that a page opens in, mapped through a connection to the X
/// server of the test's own, which reads the window's pixels back and sends
/// it input through XTest.
#[cfg(target_os = "linux")]
mod mapped {
    use std::sync::{Mutex, MutexGuard, PoisonError};
    use std::time::{Duration, Instant};

    use hapto_host::vst3::HiddenWindow;
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
    use x11rb::connection::Connection as _;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{
        BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ChangeWindowAttributesAux, ConnectionExt as _,
        EventMask, ImageFormat, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT,
    };
    use x11rb::protocol::xtest::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;

    /// The X keycode of a key, the letter A on a standard keyboard.
    const KEY: u8 = 38;

    /// Held while a test's window is mapped. Tests run at once, and each
    /// mapped window is at the screen's top left, so one would cover another.
    static SCREEN: Mutex<()> = Mutex::new(());

    pub(crate) struct Mapped {
        x: RustConnection,
        window: u32,
        root: u32,
        _screen: MutexGuard<'static, ()>,
    }

    impl Mapped {
        /// Maps `window` and selects the key presses that reach it, as a
        /// host's window does.
        pub(crate) fn new(window: &HiddenWindow) -> Mapped {
            let alone = SCREEN.lock().unwrap_or_else(PoisonError::into_inner);
            let handle = window.window_handle().expect("the window's handle");
            let RawWindowHandle::Xlib(xlib) = handle.as_raw() else {
                panic!("the test host's windows on Linux are X11 windows");
            };
            let window = u32::try_from(xlib.window).expect("an X11 window's id");
            let (x, screen) = x11rb::connect(None).expect("the X server");
            let root = x.setup().roots.get(screen).expect("the screen").root;
            let keys = ChangeWindowAttributesAux::new().event_mask(EventMask::KEY_PRESS);
            x.change_window_attributes(window, &keys)
                .expect("a request")
                .check()
                .expect("the window takes keys");
            x.map_window(window)
                .expect("a request")
                .check()
                .expect("the window maps");
            Mapped {
                x,
                window,
                root,
                _screen: alone,
            }
        }

        /// Returns how many of the pixels in the window's top left `width`
        /// by `height` are white.
        pub(crate) fn white(&self, width: u16, height: u16) -> usize {
            let image = self
                .x
                .get_image(ImageFormat::Z_PIXMAP, self.window, 0, 0, width, height, !0)
                .expect("a request")
                .reply()
                .expect("the window's pixels");
            let (pixels, _) = image.data.as_chunks::<4>();
            pixels
                .iter()
                .filter(|pixel| pixel.starts_with(&[0xff; 3]))
                .count()
        }

        /// Moves the pointer to `x`, `y` in the window.
        pub(crate) fn point_at(&self, x: i16, y: i16) {
            let at = self
                .x
                .translate_coordinates(self.window, self.root, x, y)
                .expect("a request")
                .reply()
                .expect("the window's place on the screen");
            self.fake(MOTION_NOTIFY_EVENT, 0, at.dst_x, at.dst_y);
        }

        /// Clicks where the pointer is.
        pub(crate) fn click(&self) {
            self.fake(BUTTON_PRESS_EVENT, 1, 0, 0);
            self.fake(BUTTON_RELEASE_EVENT, 1, 0, 0);
        }

        /// Presses and releases a key, and returns whether the window got
        /// the press within `patience`, while `turn` keeps the editor going.
        pub(crate) fn key_reaches_the_window(
            &self,
            patience: Duration,
            mut turn: impl FnMut(),
        ) -> bool {
            while self.x.poll_for_event().expect("the X server").is_some() {}
            self.fake(KEY_PRESS_EVENT, KEY, 0, 0);
            self.fake(KEY_RELEASE_EVENT, KEY, 0, 0);
            let end = Instant::now() + patience;
            while Instant::now() < end {
                turn();
                while let Some(event) = self.x.poll_for_event().expect("the X server") {
                    if matches!(event, Event::KeyPress(press) if press.event == self.window) {
                        return true;
                    }
                }
            }
            false
        }

        fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) {
            self.x
                .xtest_fake_input(kind, detail, 0, self.root, x, y, 0)
                .expect("a request")
                .check()
                .expect("XTest takes the input");
        }
    }

    impl Drop for Mapped {
        /// Unmaps the window before the next test may map its own.
        fn drop(&mut self) {
            if let Ok(unmap) = self.x.unmap_window(self.window) {
                let _ = unmap.check();
            }
        }
    }
}

/// Opens a probe's page in a window that the test maps, and waits until the
/// page has answered. The window drops after the view and the mapping.
#[cfg(target_os = "linux")]
fn open_mapped() -> (
    [Host<Probe>; 1],
    exporter::View<Probe>,
    mapped::Mapped,
    HiddenWindow,
) {
    let host = Host::<Probe>::new();
    let window = HiddenWindow::new().expect("a window");
    let mapped = mapped::Mapped::new(&window);
    let view = view(&host);
    assert_eq!(view.open(&window), Ok(()));
    let hosts = [host];
    let answered = until(&hosts, Duration::from_secs(30), || level(&hosts[0]) == 75.0);
    assert!(answered, "the page never answered");
    (hosts, view, mapped, window)
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "opens a real web engine in a mapped window: run it by hand"]
fn a_page_in_a_mapped_window_draws_its_pixels() {
    let (hosts, view, mapped, _window) = open_mapped();
    // The page is a line of text on WebKit's white background, so nearly
    // all of its area is white once it is drawn. A blank page, WebKitGTK's
    // known failure in virtual machines, leaves the window's own pixels
    // there instead.
    let drawn = until(&hosts, Duration::from_secs(10), || {
        mapped.white(400, 300) > 400 * 300 * 9 / 10
    });
    assert!(
        drawn,
        "only {} of the page's 120000 pixels are its white",
        mapped.white(400, 300)
    );
    view.close();
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "opens a real web engine in a mapped window and sends it input: run it by hand"]
fn a_key_over_the_page_reaches_the_host_unless_the_page_holds_the_keyboard() {
    let (hosts, view, mapped, _window) = open_mapped();
    let turn = || tick_for(&hosts, Duration::from_millis(20));
    mapped.point_at(200, 150);
    assert!(
        mapped.key_reaches_the_window(Duration::from_secs(2), turn),
        "before any click, a key over the page reaches the host"
    );
    mapped.click();
    let focused = until(&hosts, Duration::from_secs(2), || {
        has_seen("clicked: focus true")
    });
    assert!(focused, "a click gives the page the focus: {:?}", seen());
    assert!(
        !mapped.key_reaches_the_window(Duration::from_millis(500), turn),
        "after a click, the page holds the keyboard"
    );
    mapped.point_at(350, 150);
    mapped.click();
    let released = until(&hosts, Duration::from_secs(2), || {
        has_seen("handed back: focus false")
    });
    assert!(
        released,
        "the hand-back takes the focus off the page: {:?}",
        seen()
    );
    assert!(
        mapped.key_reaches_the_window(Duration::from_secs(2), turn),
        "after the hand-back, a key over the page reaches the host"
    );
    view.close();
}

#[cfg(target_os = "linux")]
#[test]
fn the_helper_is_built_under_the_name_that_the_editor_looks_for() {
    let built = std::path::Path::new(env!("CARGO_BIN_EXE_hapto-webview-helper"));
    assert_eq!(
        built.file_name(),
        Some(std::ffi::OsStr::new(hapto_webview::HELPER))
    );
}
