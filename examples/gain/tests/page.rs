//! The gain's built page in a real WebView2, in a window of the test's own
//! that is never shown: it loads from the files built into the plugin, its
//! React tree renders, and its first call, which waits for the snapshot,
//! reaches the plugin. Ignored in a normal run, since it needs the page
//! built (`npm run build` in `page/`) and the WebView2 runtime; run it by
//! hand on Windows with `cargo test -p hapto-example-gain --test page --
//! --ignored`.

#![cfg(windows)]

use std::cell::Cell;
use std::time::{Duration, Instant};

use hapto_example_gain::{GainParams, PAGE};
use hapto_host::vst3::{HiddenWindow, TestHost};
use hapto_host::{
    AuCodes, Block, Category, Editor, Layout, Parts, Plugin, PluginInfo, ProcessSetup, Processor,
    Status,
};
use hapto_webview::Page;

thread_local! {
    /// How many times the page asked for the version.
    static ASKED: Cell<u32> = const { Cell::new(0) };
}

struct Through;

impl Processor for Through {
    fn process(&mut self, _block: Block<'_, '_>) -> Status {
        Status::Continue
    }
}

/// The gain's parameters and page, with a command that records its calls.
struct Probe {
    parts: Parts,
}

impl Plugin for Probe {
    const INFO: PluginInfo = PluginInfo {
        name: "Hapto Example Gain Page Probe",
        vendor: "Hapto",
        url: "",
        email: "",
        description: "",
        version: "0.1.0",
        id: "dev.hapto.example-gain-page-probe",
        vst3_class_id: [3; 16],
        au_codes: AuCodes {
            subtype: *b"gnpp",
            manufacturer: *b"Hapt",
        },
        category: Category::Effect,
        features: &[],
        layouts: &[Layout::STEREO],
        midi_input: false,
        midi_output: false,
    };
    type Params = GainParams;
    type Processor = Through;

    fn new(_params: GainParams, parts: Parts) -> Self {
        Probe { parts }
    }

    fn activate(&mut self, _setup: &ProcessSetup) -> Through {
        Through
    }

    fn editor(&mut self) -> Option<Box<dyn Editor>> {
        let page = PAGE.expect("the page built in: run `npm run build` in examples/gain/page");
        Page::new(&Self::INFO, self.parts.ui().clone(), page)
            .command("version", |()| {
                ASKED.with(|asked| asked.set(asked.get() + 1));
                Ok(Self::INFO.version)
            })
            .into_editor()
    }
}

#[test]
#[ignore = "opens the built page in a real WebView2 in a hidden window: run by hand on Windows"]
fn the_built_page_renders_and_calls_the_plugin_once_answered() {
    let host = TestHost::<Probe>::new();
    let window = HiddenWindow::new().expect("a window");
    let view = host.view().expect("an editor");
    let started = Instant::now();
    assert_eq!(view.attach(&window), Ok(()));
    let end = started + Duration::from_secs(30);
    while ASKED.with(Cell::get) == 0 && Instant::now() < end {
        host.tick();
        let mut asked = false;
        host.pump_until(|| std::mem::replace(&mut asked, true));
    }
    println!(
        "the page's first call arrived {} ms after the open began",
        started.elapsed().as_millis()
    );
    assert!(
        ASKED.with(Cell::get) >= 1,
        "the page never called the plugin"
    );
    assert_eq!(view.remove(), Ok(()));
}
