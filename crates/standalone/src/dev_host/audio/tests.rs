//! In each format, a start the plugin refuses brings back the settings that
//! last ran, so the settings a session keeps are always ones that ran, and
//! kept settings the plugin refuses at the first start fall back to the
//! device's own.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use anyhow::{Result, anyhow};
use hapto_host::{
    AuCodes, Block, Category, Layout, Parts, Plugin, PluginInfo, ProcessSetup, Processor, Status,
};
use hapto_params::{Declarer, Params};

use super::{Audio, Device};
use crate::block::{Player, Render};
use crate::dev_host::format::Format;
use crate::dev_host::hosted::host;
use crate::settings::Settings;
use crate::signal::Signal;

/// A device at 48 kHz that takes four rates and plays nothing: a start
/// drops the player at once, and is noted.
#[derive(Default)]
struct Silent {
    starts: RefCell<Vec<u32>>,
}

impl Device for Silent {
    type Stream = ();

    fn name(&self) -> String {
        "Silent".to_owned()
    }

    fn sample_rate(&self) -> u32 {
        48_000
    }

    fn channels(&self) -> usize {
        2
    }

    fn rates(&self) -> Vec<u32> {
        vec![44_100, 48_000, 96_000, 192_000]
    }

    fn start(&self, rate: u32, player: Player<Box<dyn Render>>) -> Result<()> {
        drop(player);
        self.starts.borrow_mut().push(rate);
        Ok(())
    }

    fn ended((): &()) -> Option<String> {
        None
    }
}

/// A device at 48 kHz that refuses every start.
struct Broken;

impl Device for Broken {
    type Stream = ();

    fn name(&self) -> String {
        "Broken".to_owned()
    }

    fn sample_rate(&self) -> u32 {
        48_000
    }

    fn channels(&self) -> usize {
        2
    }

    fn rates(&self) -> Vec<u32> {
        vec![48_000]
    }

    fn start(&self, _rate: u32, _player: Player<Box<dyn Render>>) -> Result<()> {
        Err(anyhow!("the device is in use by another app"))
    }

    fn ended((): &()) -> Option<String> {
        None
    }
}

/// A device at 48 kHz whose stream a test can end, as unplugging it does.
#[derive(Default)]
struct Unpluggable {
    stream: RefCell<Option<Rc<RefCell<Option<String>>>>>,
}

impl Unpluggable {
    /// Ends the stream playing now, for `why`.
    fn unplug(&self, why: &str) {
        if let Some(stream) = &*self.stream.borrow() {
            *stream.borrow_mut() = Some(why.to_owned());
        }
    }
}

impl Device for Unpluggable {
    type Stream = Rc<RefCell<Option<String>>>;

    fn name(&self) -> String {
        "Unpluggable".to_owned()
    }

    fn sample_rate(&self) -> u32 {
        48_000
    }

    fn channels(&self) -> usize {
        2
    }

    fn rates(&self) -> Vec<u32> {
        vec![48_000]
    }

    fn start(&self, _rate: u32, _player: Player<Box<dyn Render>>) -> Result<Self::Stream> {
        let stream = Rc::new(RefCell::new(None));
        *self.stream.borrow_mut() = Some(Rc::clone(&stream));
        Ok(stream)
    }

    fn ended(stream: &Self::Stream) -> Option<String> {
        stream.borrow().clone()
    }
}

thread_local! {
    /// Whether the watched plugin is active.
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    /// Whether the watched plugin refuses every activation.
    static REFUSES: Cell<bool> = const { Cell::new(false) };
}

/// A plugin that says whether it is active, and refuses every activation
/// while asked to.
struct Watched;

impl Plugin for Watched {
    const INFO: PluginInfo = PluginInfo {
        name: "Watched",
        id: "dev.test.watched",
        vst3_class_id: [4; 16],
        ..Picky::INFO
    };
    type Params = NoParams;
    type Processor = Through;

    fn new(_params: NoParams, _parts: Parts) -> Self {
        Watched
    }

    fn activate(&mut self, _setup: &ProcessSetup) -> Through {
        assert!(!REFUSES.get(), "refuses every activation");
        ACTIVE.set(true);
        Through
    }

    fn deactivate(&mut self, _processor: Through) {
        ACTIVE.set(false);
    }
}

#[derive(Clone)]
struct NoParams;

impl Params for NoParams {
    fn declare(_: &mut Declarer) -> Self {
        NoParams
    }
}

struct Through;

impl Processor for Through {
    fn process(&mut self, _block: Block<'_, '_>) -> Status {
        Status::Continue
    }
}

/// A plugin whose activation above 100 kHz panics, which each format
/// catches and answers as a refusal.
struct Picky;

impl Plugin for Picky {
    const INFO: PluginInfo = PluginInfo {
        name: "Picky",
        vendor: "Test",
        url: "",
        email: "",
        description: "",
        version: "0.1.0",
        id: "dev.test.picky",
        vst3_class_id: [3; 16],
        au_codes: AuCodes {
            subtype: *b"pick",
            manufacturer: *b"Test",
        },
        category: Category::Effect,
        features: &[],
        layouts: &[Layout::STEREO],
        midi_input: false,
        midi_output: false,
    };
    type Params = NoParams;
    type Processor = Through;

    fn new(_params: NoParams, _parts: Parts) -> Self {
        Picky
    }

    fn activate(&mut self, setup: &ProcessSetup) -> Through {
        assert!(setup.sample_rate <= 100_000.0, "no rate above 100 kHz");
        Through
    }
}

const OWN: Settings = Settings {
    rate: 48_000,
    max: 512,
    blocks: crate::settings::Blocks::Device,
};

#[test]
fn a_start_the_plugin_refuses_brings_back_the_settings_that_last_ran() {
    for format in Format::ALL {
        let hosted = host::<Picky>(format);
        let mut audio = Audio::with(Silent::default(), Layout::STEREO, Signal::Silence, None);
        audio.start_first(&*hosted);
        assert_eq!(audio.running(), Some(OWN), "{format:?}");
        audio.stop(&*hosted);
        let faster = Settings {
            rate: 192_000,
            ..OWN
        };
        audio.change(&*hosted, faster);
        assert_eq!(audio.running(), None, "{format:?}: stopped, none to keep");
        audio.resume(&*hosted);
        assert_eq!(audio.running(), Some(OWN), "{format:?}: the ones that ran");
        let note = audio.shown().note.map(str::to_owned).unwrap_or_default();
        assert!(
            note.contains("settings before are back"),
            "{format:?}: {note}"
        );
    }
}

#[test]
fn kept_settings_the_plugin_refuses_fall_back_to_the_device_s_own() {
    for format in Format::ALL {
        let hosted = host::<Picky>(format);
        let kept = Some("192000 512 device");
        let mut audio = Audio::with(Silent::default(), Layout::STEREO, Signal::Silence, kept);
        assert_eq!(audio.settings().rate, 192_000, "the kept ones");
        audio.start_first(&*hosted);
        assert_eq!(audio.running(), Some(OWN), "{format:?}");
    }
}

#[test]
fn a_plugin_refused_even_at_the_device_s_own_settings_still_lets_the_dev_host_start() {
    for format in Format::ALL {
        let hosted = host::<Watched>(format);
        REFUSES.set(true);
        let kept = Some("96000 512 device");
        let mut audio = Audio::with(Silent::default(), Layout::STEREO, Signal::Silence, kept);
        audio.start_first(&*hosted);
        REFUSES.set(false);
        assert_eq!(audio.running(), None, "{format:?}: stopped");
        let note = audio.shown().note.map(str::to_owned).unwrap_or_default();
        assert!(note.contains("did not start either"), "{format:?}: {note}");
        assert!(note.contains("processing is stopped"), "{format:?}: {note}");
        audio.resume(&*hosted);
        assert_eq!(
            audio.running(),
            Some(OWN),
            "{format:?}: starts once it takes"
        );
    }
}

#[test]
fn a_device_that_does_not_start_leaves_the_plugin_stopped() {
    for format in Format::ALL {
        let hosted = host::<Watched>(format);
        let mut audio = Audio::with(Broken, Layout::STEREO, Signal::Silence, None);
        audio.start_first(&*hosted);
        assert!(!ACTIVE.get(), "{format:?}: deactivated again");
        assert_eq!(audio.running(), None, "{format:?}");
        let note = audio.shown().note.map(str::to_owned).unwrap_or_default();
        assert!(note.contains("in use by another app"), "{format:?}: {note}");
    }
}

#[test]
fn a_stream_that_ends_on_its_own_stops_processing_and_says_why() {
    for format in Format::ALL {
        let hosted = host::<Watched>(format);
        let mut audio = Audio::with(
            Unpluggable::default(),
            Layout::STEREO,
            Signal::Silence,
            None,
        );
        audio.start_first(&*hosted);
        assert_eq!(audio.running(), Some(OWN), "{format:?}");
        audio.check(&*hosted);
        assert_eq!(audio.running(), Some(OWN), "{format:?}: still playing");
        audio.output().unplug("the device was unplugged");
        audio.check(&*hosted);
        assert_eq!(audio.running(), None, "{format:?}: stopped");
        assert!(!ACTIVE.get(), "{format:?}: deactivated");
        let note = audio.shown().note.map(str::to_owned).unwrap_or_default();
        assert!(note.contains("unplugged"), "{format:?}: {note}");
        assert!(
            audio
                .events()
                .iter()
                .any(|event| event.contains("unplugged")),
            "{format:?}: in the log"
        );
    }
}
