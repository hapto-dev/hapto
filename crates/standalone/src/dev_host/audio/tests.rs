//! In each format, a start the plugin refuses brings back the settings that
//! last ran, so the settings a session keeps are always ones that ran, and
//! kept settings the plugin refuses at the first start fall back to the
//! device's own. A device error the device plays on after is logged, and any
//! other stops the plugin until the next look. A look follows the system's
//! default output: processing, to a new device, where the plugin is started
//! again, or to none, where it waits; waiting, it starts the plugin again
//! once it can; stopped by the author, it takes the new device and starts
//! nothing. A wait ends once the plugin plays again, and at a Stop, a Start
//! or a Reactivate, and a move clears the note of an earlier refusal.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use anyhow::{Context as _, Result, anyhow, bail};
use cpal::ErrorKind;
use hapto_host::{
    AuCodes, Block, Category, Layout, Parts, Plugin, PluginInfo, ProcessSetup, Processor, Status,
};
use hapto_params::{Declarer, Params};

use super::{Audio, Device};
use crate::block::{Player, Render};
use crate::dev_host::format::Format;
use crate::dev_host::hosted::{Hosted, host};
use crate::device::DeviceError;
use crate::settings::{Blocks, Settings};
use crate::signal::Signal;

/// A device at 48 kHz that takes four rates and plays nothing: a start
/// drops the player at once, and is noted.
#[derive(Default)]
struct Silent {
    starts: RefCell<Vec<u32>>,
}

impl Device for Silent {
    type Stream = ();
    type Id = ();

    fn name(&self) -> String {
        "Silent".to_owned()
    }

    fn id(&self) -> &() {
        &()
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

    fn default_id(&self) -> Option<()> {
        Some(())
    }

    fn reopen(&self) -> Result<Silent> {
        Ok(Silent::default())
    }
}

/// A device at 48 kHz that refuses every start.
struct Broken;

impl Device for Broken {
    type Stream = ();
    type Id = ();

    fn name(&self) -> String {
        "Broken".to_owned()
    }

    fn id(&self) -> &() {
        &()
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

    fn default_id(&self) -> Option<()> {
        Some(())
    }

    fn reopen(&self) -> Result<Broken> {
        Ok(Broken)
    }
}

/// What a test's system says of its outputs, shared by every device opened
/// from it, as the system's own state is.
struct System {
    /// The number of the default output; `None` when there is none.
    default: Cell<Option<u32>>,
    /// The rate the device opened next plays at.
    next_rate: Cell<u32>,
    /// Whether every start fails, as on a device another app holds.
    busy: Cell<bool>,
    /// How many of the next starts fail, as on a device another app holds
    /// for a moment.
    refusals: Cell<u32>,
}

/// A device numbered as the system's default output was when it opened,
/// whose stream a test can have report an error.
struct Changing {
    number: u32,
    rate: u32,
    system: Rc<System>,
    stream: RefCell<Option<Rc<RefCell<Option<DeviceError>>>>>,
}

impl Changing {
    /// Opens the system's default output.
    fn opened(system: &Rc<System>) -> Result<Changing> {
        let number = system
            .default
            .get()
            .context("there is no audio output device")?;
        Ok(Changing {
            number,
            rate: system.next_rate.get(),
            system: Rc::clone(system),
            stream: RefCell::new(None),
        })
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
    type Id = u32;

    fn name(&self) -> String {
        format!("Device {}", self.number)
    }

    fn id(&self) -> &u32 {
        &self.number
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
        let refusals = self.system.refusals.get();
        if self.system.busy.get() || refusals > 0 {
            self.system.refusals.set(refusals.saturating_sub(1));
            bail!("the device is in use by another app");
        }
        let stream = Rc::new(RefCell::new(None));
        *self.stream.borrow_mut() = Some(Rc::clone(&stream));
        Ok(stream)
    }

    fn error(stream: &mut Self::Stream) -> Option<DeviceError> {
        stream.borrow_mut().take()
    }

    fn default_id(&self) -> Option<u32> {
        self.system.default.get()
    }

    fn reopen(&self) -> Result<Changing> {
        Changing::opened(&self.system)
    }
}

/// A system whose default output is Device 1, at 48 kHz.
fn system() -> Rc<System> {
    Rc::new(System {
        default: Cell::new(Some(1)),
        next_rate: Cell::new(48_000),
        busy: Cell::new(false),
        refusals: Cell::new(0),
    })
}

/// The audio of a plugin started through `hosted` on `system`'s default
/// output, with `kept` settings, and the start's log taken.
fn started(hosted: &dyn Hosted, system: &Rc<System>, kept: Option<&str>) -> Audio<Changing> {
    let device = Changing::opened(system).expect("a default");
    let mut audio = Audio::with(device, Layout::STEREO, Signal::Silence, kept);
    audio.start_first(hosted);
    let _started = audio.events();
    audio
}

thread_local! {
    /// Whether the watched plugin is active.
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    /// How many times the watched plugin was activated.
    static ACTIVATIONS: Cell<u32> = const { Cell::new(0) };
    /// Whether the watched plugin refuses every activation.
    static REFUSES: Cell<bool> = const { Cell::new(false) };
}

/// A plugin that says whether it is active, and how many times it was, and
/// refuses every activation while asked to.
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
        ACTIVATIONS.set(ACTIVATIONS.get() + 1);
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
    blocks: Blocks::Device,
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

/// The note the Audio tab shows, empty for none.
fn note<D: Device>(audio: &Audio<D>) -> String {
    audio.shown().note.map(str::to_owned).unwrap_or_default()
}

#[test]
fn an_error_the_device_plays_on_after_is_only_logged() {
    for format in Format::ALL {
        let hosted = host::<Watched>(format);
        let mut audio = started(&*hosted, &system(), None);
        for kind in [ErrorKind::Xrun, ErrorKind::RealtimeDenied] {
            audio.output().report(kind);
            audio.check(&*hosted);
            assert_eq!(audio.running(), Some(OWN), "{format:?}: plays on");
            assert!(ACTIVE.get(), "{format:?}: still active");
            assert_eq!(audio.shown().device, "Device 1", "{format:?}");
            let events = audio.events();
            assert!(logged(&events, "plays on"), "{format:?}: {events:?}");
        }
    }
}

#[test]
fn a_new_default_while_processing_moves_the_plugin_there_and_reactivates_it() {
    for format in Format::ALL {
        let hosted = host::<Watched>(format);
        let system = system();
        let mut audio = started(&*hosted, &system, None);
        let activations = ACTIVATIONS.get();
        system.default.set(Some(2));
        audio.look(&*hosted);
        assert_eq!(audio.running(), Some(OWN), "{format:?}");
        assert!(ACTIVE.get(), "{format:?}: active");
        assert_eq!(ACTIVATIONS.get(), activations + 1, "{format:?}: again");
        assert_eq!(audio.shown().device, "Device 2", "{format:?}");
        assert_eq!(note(&audio), "", "{format:?}: a move leaves no note");
        let events = audio.events();
        assert!(logged(&events, "now Device 2"), "{format:?}: {events:?}");
        assert!(logged(&events, "activated at"), "{format:?}: {events:?}");
    }
}

#[test]
fn a_new_default_that_lacks_the_rate_takes_its_own_and_the_settings_are_kept() {
    for format in Format::ALL {
        let hosted = host::<Watched>(format);
        let system = system();
        let mut audio = started(&*hosted, &system, Some("48000 256 random"));
        system.default.set(Some(2));
        system.next_rate.set(44_100);
        assert!(audio.look(&*hosted), "{format:?}: for the session to keep");
        let moved = Settings {
            rate: 44_100,
            max: 256,
            blocks: Blocks::Random,
        };
        assert_eq!(audio.running(), Some(moved), "{format:?}: its own rate");
        assert!(!audio.look(&*hosted), "{format:?}: nothing new to keep");
    }
}

#[test]
fn an_error_drops_the_stream_and_the_next_look_opens_the_default() {
    for format in Format::ALL {
        let hosted = host::<Watched>(format);
        let mut audio = started(&*hosted, &system(), None);
        // A second error soon after the first waits for the look too.
        for time in ["first", "second"] {
            audio.output().report(ErrorKind::StreamInvalidated);
            audio.check(&*hosted);
            assert_eq!(audio.running(), None, "{format:?} {time}: dropped");
            assert!(!ACTIVE.get(), "{format:?} {time}: deactivated");
            let stopped = note(&audio);
            assert!(stopped.contains("processing is stopped"), "{stopped}");
            audio.look(&*hosted);
            assert_eq!(audio.running(), Some(OWN), "{format:?} {time}: plays");
            assert!(ACTIVE.get(), "{format:?} {time}: active again");
            assert_eq!(note(&audio), "", "{format:?} {time}: cleared");
        }
    }
}

#[test]
fn no_default_stops_processing_with_the_note_and_a_new_one_starts_it_again() {
    for format in Format::ALL {
        let hosted = host::<Watched>(format);
        let system = system();
        let mut audio = started(&*hosted, &system, None);
        system.default.set(None);
        audio.look(&*hosted);
        assert_eq!(audio.running(), None, "{format:?}: stopped");
        assert!(!ACTIVE.get(), "{format:?}: deactivated");
        let stopped = note(&audio);
        assert!(stopped.contains("no audio output device"), "{stopped}");
        assert!(stopped.contains("processing is stopped"), "{stopped}");
        assert!(logged(&audio.events(), "no audio output device"));
        audio.look(&*hosted);
        assert_eq!(audio.events(), Vec::<String>::new(), "{format:?}: waits");
        system.default.set(Some(3));
        audio.look(&*hosted);
        assert_eq!(audio.running(), Some(OWN), "{format:?}: started");
        assert!(ACTIVE.get(), "{format:?}: active");
        assert_eq!(audio.shown().device, "Device 3", "{format:?}");
        assert_eq!(note(&audio), "", "{format:?}: cleared");
    }
}

#[test]
fn a_start_that_fails_after_a_move_is_tried_at_each_look_and_logged_once() {
    for format in Format::ALL {
        let hosted = host::<Watched>(format);
        let system = system();
        let mut audio = started(&*hosted, &system, None);
        system.default.set(Some(2));
        system.busy.set(true);
        audio.look(&*hosted);
        assert_eq!(audio.running(), None, "{format:?}: stopped");
        assert!(!ACTIVE.get(), "{format:?}: deactivated again");
        assert!(note(&audio).contains("in use"), "{format:?}");
        let events = audio.events();
        let refusals = events.iter().filter(|e| e.contains("in use")).count();
        assert_eq!(refusals, 1, "{format:?}: {events:?}");
        audio.look(&*hosted);
        assert!(!logged(&audio.events(), "in use"), "{format:?}: once");
        assert!(note(&audio).contains("in use"), "{format:?}: still noted");
        system.busy.set(false);
        audio.look(&*hosted);
        assert_eq!(audio.running(), Some(OWN), "{format:?}: tried again");
        assert!(ACTIVE.get(), "{format:?}: active");
        assert_eq!(note(&audio), "", "{format:?}: cleared");
    }
}

#[test]
fn after_stop_a_new_default_is_taken_and_not_started_and_start_plays_on_it() {
    for format in Format::ALL {
        let hosted = host::<Watched>(format);
        let system = system();
        let mut audio = started(&*hosted, &system, None);
        audio.stop(&*hosted);
        system.default.set(Some(2));
        system.next_rate.set(44_100);
        audio.look(&*hosted);
        assert_eq!(audio.running(), None, "{format:?}: not started");
        assert!(!ACTIVE.get(), "{format:?}: inactive");
        assert_eq!(audio.shown().device, "Device 2", "{format:?}");
        assert_eq!(audio.shown().rates, [44_100], "{format:?}: its rates");
        assert!(logged(&audio.events(), "now Device 2"), "{format:?}");
        audio.resume(&*hosted);
        let there = Settings {
            rate: 44_100,
            ..OWN
        };
        assert_eq!(audio.running(), Some(there), "{format:?}: plays on it");
        assert_eq!(audio.output().number, 2, "{format:?}");
    }
}

#[test]
fn a_stop_a_start_or_a_reactivation_ends_the_wait_for_a_device() {
    type End = fn(&mut Audio<Changing>, &dyn Hosted);
    let ends: [(&str, End); 3] = [
        ("Stop", Audio::stop),
        ("Start", Audio::resume),
        ("Reactivate", Audio::reactivate),
    ];
    for format in Format::ALL {
        for (name, end) in ends {
            let hosted = host::<Watched>(format);
            let system = system();
            let mut audio = started(&*hosted, &system, None);
            audio.output().report(ErrorKind::StreamInvalidated);
            audio.check(&*hosted);
            // The device is held, so a start asked for now fails.
            system.busy.set(true);
            end(&mut audio, &*hosted);
            assert_eq!(audio.running(), None, "{format:?} {name}: stopped");
            system.busy.set(false);
            let activations = ACTIVATIONS.get();
            audio.look(&*hosted);
            assert_eq!(audio.running(), None, "{format:?} {name}: no more tries");
            assert_eq!(ACTIVATIONS.get(), activations, "{format:?} {name}");
        }
    }
}

#[test]
fn once_the_plugin_plays_again_a_later_refusal_is_not_tried_at_a_look() {
    for format in Format::ALL {
        for by_a_move in [false, true] {
            let hosted = host::<Watched>(format);
            let system = system();
            let mut audio = started(&*hosted, &system, None);
            if by_a_move {
                system.default.set(Some(2));
            } else {
                audio.output().report(ErrorKind::StreamInvalidated);
                audio.check(&*hosted);
            }
            audio.look(&*hosted);
            assert_eq!(audio.running(), Some(OWN), "{format:?}: plays again");
            // A change the device refuses, and the settings before with it.
            system.busy.set(true);
            let smaller = Settings { max: 256, ..OWN };
            audio.change(&*hosted, smaller);
            assert_eq!(audio.running(), None, "{format:?}: refused");
            system.busy.set(false);
            let activations = ACTIVATIONS.get();
            audio.look(&*hosted);
            assert_eq!(audio.running(), None, "{format:?}: not tried again");
            assert_eq!(ACTIVATIONS.get(), activations, "{format:?}");
        }
    }
}

#[test]
fn a_move_clears_the_note_of_an_earlier_refusal() {
    for format in Format::ALL {
        let hosted = host::<Watched>(format);
        let system = system();
        let mut audio = started(&*hosted, &system, None);
        // One start fails: the change is refused, and the settings before
        // come back with a note.
        system.refusals.set(1);
        let smaller = Settings { max: 256, ..OWN };
        audio.change(&*hosted, smaller);
        assert_eq!(audio.running(), Some(OWN), "{format:?}: the ones before");
        let refused = note(&audio);
        assert!(refused.contains("settings before are back"), "{refused}");
        system.default.set(Some(2));
        audio.look(&*hosted);
        assert_eq!(audio.shown().device, "Device 2", "{format:?}");
        assert_eq!(note(&audio), "", "{format:?}: a move leaves no note");
    }
}
