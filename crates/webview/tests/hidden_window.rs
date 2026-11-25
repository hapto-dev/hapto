//! The page in a real WebView2, in a window of the test's own that is never
//! shown, through the VST3 exporter as a host drives it: the page loads from
//! the files built into the plugin, its `ready` is answered with a
//! snapshot, its `set` reaches the parameter and its `setValue` the value,
//! a native change to the value reaches it at the tick, and it cannot
//! navigate away.
//! Ignored in a normal run, since it needs the WebView2 runtime and starts
//! its browser processes; run it by hand on Windows with `cargo test -p
//! hapto-webview --test hidden_window -- --ignored`.
//!
//! It never shows a window of its own, takes focus or sends input. The page's
//! script drives each test. One test does make WebView2 show a dialog, as
//! that test's doc says.

#![cfg(windows)]

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use hapto_host::layout::{AnyLayout, Width};
use hapto_host::vst3::{HiddenWindow, TestHost};
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
/// label the native side sends it after the snapshot.
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

/// A plugin whose page cannot start: a file stands where the WebView would
/// keep its data.
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
        let base = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA is set");
        let path = PathBuf::from(base).join(id);
        std::fs::write(&path, b"").expect("a file where the data folder would be");
        Blocker(path)
    }
}

impl Drop for Blocker {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Ticks `hosts` and runs this thread's message loop until `done` holds,
/// for at most `patience`; returns whether it holds.
fn until(hosts: &[TestHost<Probe>], patience: Duration, mut done: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + patience;
    while Instant::now() < end {
        for host in hosts {
            host.tick();
        }
        if done() {
            return true;
        }
        // Runs the message loop for about 10 ms, as a host's would: the
        // second ask ends it.
        let mut asked = false;
        hosts[0].pump_until(|| std::mem::replace(&mut asked, true));
    }
    done()
}

/// Keeps the editors open, ticking, for 20 seconds when `HAPTO_HOLD_OPEN`
/// is set: time to read their processes and memory from outside.
fn hold_open(hosts: &[TestHost<Probe>]) {
    if std::env::var_os("HAPTO_HOLD_OPEN").is_some() {
        until(hosts, Duration::from_secs(20), || false);
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

fn level(host: &TestHost<Probe>) -> f64 {
    host.plain(
        hapto_params::host_id("level"),
        host.normalized(hapto_params::host_id("level")),
    )
}

#[test]
#[ignore = "opens a real WebView2 in a hidden window: run by hand on Windows"]
fn a_page_in_a_hidden_window_is_answered_and_its_set_reaches_the_parameter() {
    let host = TestHost::<Probe>::new();
    let window = HiddenWindow::new().expect("a window");
    let view = host.view().expect("an editor");
    let started = Instant::now();
    assert_eq!(view.attach(&window), Ok(()));
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
    // answered, and the page's navigation was cancelled, so it stayed to say
    // so.
    assert_eq!(seen()[..2], ["50.00 %", "label Init"]);
    assert_eq!(label().get(), "Page");
    // The plugin renames itself: the next tick tells the page.
    label().set(String::from("Native"));
    let heard = until(&hosts, Duration::from_secs(10), || has_seen("label Native"));
    assert!(heard, "the page never heard the plugin's rename");
    let first = seen();
    assert_eq!(first.len(), 5, "{first:?}");
    assert_eq!(view.remove(), Ok(()));
    // Closed: ticks reach no page.
    label().set(String::from("Closed"));
    until(&hosts, Duration::from_millis(200), || false);
    assert_eq!(seen().len(), 5);
    // Opened again: a new page asks for its snapshot and reports it.
    assert_eq!(view.attach(&window), Ok(()));
    let reopened = until(&hosts, Duration::from_secs(30), || {
        seen().len() == 9 && seen().iter().filter(|seen| *seen == "stayed").count() == 2
    });
    assert!(reopened, "the reopened page never reported: {:?}", seen());
    assert_eq!(seen()[5..7], ["75.00 %", "label Closed"]);
    assert_eq!(view.remove(), Ok(()));
    assert_eq!(window.shown_children(), 0, "closed, it left nothing");
    drop(view);
}

/// WebView2 can't create its data folder here, so it shows its own error
/// dialog, which says that it couldn't create the data directory. The dialog
/// belongs to WebView2's browser process and stays open after the test ends.
/// Click OK to dismiss it. The test passes whether or not you dismiss it.
#[test]
#[ignore = "fails to start a real WebView2 in a hidden window: run by hand on Windows"]
fn a_page_that_cannot_start_leaves_nothing_shown_in_the_window() {
    let _blocker = Blocker::new(Unstartable::INFO.id);
    let host = TestHost::<Unstartable>::new();
    let window = HiddenWindow::new().expect("a window");
    let view = host.view().expect("an editor");
    assert!(
        view.attach(&window).is_err(),
        "the WebView has no data folder"
    );
    host.expect_faults(1);
    assert_eq!(window.shown_children(), 0);
    drop(view);
}

#[test]
#[ignore = "opens sixteen real WebView2s in hidden windows: run by hand on Windows"]
fn sixteen_editors_open_in_one_process() {
    let hosts: Vec<TestHost<Probe>> = (0..16).map(|_| TestHost::<Probe>::new()).collect();
    let windows: Vec<HiddenWindow> = (0..16)
        .map(|_| HiddenWindow::new().expect("a window"))
        .collect();
    let views: Vec<_> = hosts
        .iter()
        .map(|host| host.view().expect("an editor"))
        .collect();
    let started = Instant::now();
    for (view, window) in views.iter().zip(&windows) {
        assert_eq!(view.attach(window), Ok(()));
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
        assert_eq!(view.remove(), Ok(()));
    }
}
