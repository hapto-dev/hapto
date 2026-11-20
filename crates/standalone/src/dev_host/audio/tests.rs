//! In each format, a start the plugin refuses brings back the settings that
//! last ran, so the settings a session keeps are always ones that ran, and
//! kept settings the plugin refuses at the first start fall back to the
//! device's own. A device error the device plays on after is logged, and
//! any other opens the default output again and starts the plugin there.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use cpal::ErrorKind;
use hapto_host::{
    AuCodes, Block, Category, Layout, Parts, Plugin, PluginInfo, ProcessSetup, Processor, Status,
};
use hapto_params::{Declarer, Params};

use super::{Audio, Device};
use crate::block::{Player, Render};
use crate::dev_host::format::Format;
use crate::dev_host::hosted::host;
use crate::device::{DeviceError, SETTLE};
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

    fn error((): &mut ()) -> Option<DeviceError> {
        None
    }

    fn reopen(&self) -> Result<Silent> {
        Ok(Silent::default())
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

    fn error((): &mut ()) -> Option<DeviceError> {
        None
    }

    fn reopen(&self) -> Result<Broken> {
        Ok(Broken)
    }
}

/// A device whose stream a test can have report an error, as a change of
/// the system's default output does. It opens again as a fresh copy of
/// itself, numbered one on, at `next_rate`, unless it is told none is left.
struct Changing {
    number: u32,
    rate: u32,
    /// The rate the next copy plays at.
    next_rate: Cell<u32>,
    /// Whether no default output is left to open.
    none_left: Cell<bool>,
    stream: RefCell<Option<Rc<RefCell<Option<DeviceError>>>>>,
}

impl Default for Changing {
    fn default() -> Self {
        Changing::numbered(1, 48_000)
    }
}

impl Changing {
    fn numbered(number: u32, rate: u32) -> Changing {
        Changing {
            number,
            rate,
            next_rate: Cell::new(rate),
            none_left: Cell::new(false),
            stream: RefCell::new(None),
        }
    }

    /// Has the stream playing now report an error of `kind`.
    fn report(&self, kind: ErrorKind) {
        if let Some(stream) = &*self.stream.borrow() {
            *stream.borrow_mut() = Some(DeviceError {
                kind,
                text: format!("{kind:?}"),
            });
        }
    }
}

impl Device for Changing {
    type Stream = Rc<RefCell<Option<DeviceError>>>;

    fn name(&self) -> String {
        format!("Device {}", self.number)
    }

    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn channels(&self) -> usize {
        2
    }

    fn rates(&self) -> Vec<u32> {
        vec![self.rate]
    }

    fn start(&self, _rate: u32, _player: Player<Box<dyn Render>>) -> Result<Self::Stream> {
        let stream = Rc::new(RefCell::new(None));
        *self.stream.borrow_mut() = Some(Rc::clone(&stream));
        Ok(stream)
    }

    fn error(stream: &mut Self::Stream) -> Option<DeviceError> {
        stream.borrow_mut().take()
    }

    fn reopen(&self) -> Result<Changing> {
        if self.none_left.get() {
            bail!("there is no audio output device");
        }
        Ok(Changing::numbered(self.number + 1, self.next_rate.get()))
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

/// Whether any of `events` holds `text`.
fn logged(events: &[String], text: &str) -> bool {
    events.iter().any(|event| event.contains(text))
}

#[test]
fn an_error_the_device_plays_on_after_is_only_logged() {
    for format in Format::ALL {
        let hosted = host::<Watched>(format);
        let mut audio = Audio::with(Changing::default(), Layout::STEREO, Signal::Silence, None);
        audio.start_first(&*hosted);
        let _started = audio.events();
        for kind in [ErrorKind::Xrun, ErrorKind::RealtimeDenied] {
            audio.output().report(kind);
            audio.check(&*hosted, Instant::now());
            assert_eq!(audio.running(), Some(OWN), "{format:?}: plays on");
            assert!(ACTIVE.get(), "{format:?}: still active");
            assert_eq!(audio.shown().device, "Device 1", "{format:?}");
            let events = audio.events();
            assert!(logged(&events, "plays on"), "{format:?}: {events:?}");
        }
    }
}

#[test]
fn a_new_default_output_reopens_and_reactivates_the_plugin_there() {
    for format in Format::ALL {
        let hosted = host::<Watched>(format);
        let mut audio = Audio::with(Changing::default(), Layout::STEREO, Signal::Silence, None);
        audio.start_first(&*hosted);
        let _started = audio.events();
        audio.output().next_rate.set(44_100);
        audio.output().report(ErrorKind::StreamInvalidated);
        audio.check(&*hosted, Instant::now());
        let at_its_own = Settings {
            rate: 44_100,
            ..OWN
        };
        assert_eq!(audio.running(), Some(at_its_own), "{format:?}: its rate");
        assert!(ACTIVE.get(), "{format:?}: active again");
        assert_eq!(audio.shown().device, "Device 2", "{format:?}");
        let note = audio.shown().note.map(str::to_owned).unwrap_or_default();
        assert!(note.contains("Device 2"), "{format:?}: {note}");
        let events = audio.events();
        assert!(logged(&events, "now Device 2"), "{format:?}: {events:?}");
        assert!(logged(&events, "activated at"), "{format:?}: {events:?}");
    }
}

#[test]
fn a_default_output_that_does_not_open_again_stops_processing_with_the_note() {
    for format in Format::ALL {
        let hosted = host::<Watched>(format);
        let mut audio = Audio::with(Changing::default(), Layout::STEREO, Signal::Silence, None);
        audio.start_first(&*hosted);
        audio.output().none_left.set(true);
        audio.output().report(ErrorKind::DeviceNotAvailable);
        audio.check(&*hosted, Instant::now());
        assert_eq!(audio.running(), None, "{format:?}: stopped");
        assert!(!ACTIVE.get(), "{format:?}: deactivated");
        let note = audio.shown().note.map(str::to_owned).unwrap_or_default();
        assert!(
            note.contains("no audio output device"),
            "{format:?}: {note}"
        );
        assert!(note.contains("processing is stopped"), "{format:?}: {note}");
        assert!(logged(&audio.events(), "did not open again"), "{format:?}");
    }
}

#[test]
fn a_device_that_fails_again_at_once_is_not_opened_again() {
    for format in Format::ALL {
        let hosted = host::<Watched>(format);
        let mut audio = Audio::with(Changing::default(), Layout::STEREO, Signal::Silence, None);
        audio.start_first(&*hosted);
        let start = Instant::now();
        audio.output().report(ErrorKind::StreamInvalidated);
        audio.check(&*hosted, start);
        assert_eq!(audio.shown().device, "Device 2", "{format:?}");
        audio.output().report(ErrorKind::StreamInvalidated);
        audio.check(&*hosted, start + SETTLE - Duration::from_millis(1));
        assert_eq!(audio.running(), None, "{format:?}: stopped");
        assert!(!ACTIVE.get(), "{format:?}: deactivated");
        assert_eq!(audio.shown().device, "Device 2", "{format:?}: not opened");
        let note = audio.shown().note.map(str::to_owned).unwrap_or_default();
        assert!(note.contains("within a second"), "{format:?}: {note}");
    }
}
