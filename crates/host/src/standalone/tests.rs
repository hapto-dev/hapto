//! The standalone exporter: a block before and after an activation, the
//! editor in the app's window, the saved block between runs, and a teardown
//! that never frees the plugin where an audio side could still reach it.

use std::cell::{Cell, RefCell};
use std::error::Error;
use std::num::NonZeroIsize;

use hapto_params::{Declarer, Float, FloatDecl, Params, Value};
use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};

use super::{LoadRefused, MAX_CHANNELS, OpenError, Refused, Standalone};
use crate::faults::{thread_faults, written};
use crate::layout::{AnyLayout, NoAudio, Stereo, Width};
use crate::{
    AuCodes, Block, Category, Editor, Event, LoadError, ParentWindow, Parts, Plugin, PluginInfo,
    ProcessSetup, Processor, Size, Status, Tail,
};

/// What the stub editor was asked to do, in order.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Call {
    Scale(f64),
    Size(u32, u32),
    Open,
    Close,
    Tick,
}

thread_local! {
    static CALLS: RefCell<Vec<Call>> = const { RefCell::new(Vec::new()) };
    static HAS_EDITOR: Cell<bool> = const { Cell::new(false) };
    static OPEN_FAILS: Cell<bool> = const { Cell::new(false) };
    /// What runs inside the next `open`, as an app's nested message loop
    /// would.
    static DURING_OPEN: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    /// The reason an editor that fails gives cannot be written.
    static REASON_PANICS: Cell<bool> = const { Cell::new(false) };
    static EDITOR_DROP_PANICS: Cell<bool> = const { Cell::new(false) };
    static PLUGINS_DROPPED: Cell<u32> = const { Cell::new(0) };
    static PLUGIN_DROP_PANICS: Cell<bool> = const { Cell::new(false) };
}

/// A reason whose text panics as it is written.
#[derive(Debug)]
struct Unwritable;

impl std::fmt::Display for Unwritable {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        panic!("the reason cannot be written");
    }
}

impl Error for Unwritable {}

fn calls() -> Vec<Call> {
    CALLS.with_borrow_mut(std::mem::take)
}

struct Stub;

impl Editor for Stub {
    fn size(&self) -> Size {
        Size {
            width: 300,
            height: 200,
        }
    }

    fn open(&mut self, _parent: &ParentWindow) -> Result<(), Box<dyn Error>> {
        CALLS.with_borrow_mut(|calls| calls.push(Call::Open));
        if let Some(during) = DURING_OPEN.with_borrow_mut(Option::take) {
            during();
        }
        if REASON_PANICS.get() {
            return Err(Box::new(Unwritable));
        }
        if OPEN_FAILS.get() {
            return Err("no toolkit here".into());
        }
        Ok(())
    }

    fn close(&mut self) {
        CALLS.with_borrow_mut(|calls| calls.push(Call::Close));
    }

    fn set_size(&mut self, size: Size) {
        CALLS.with_borrow_mut(|calls| calls.push(Call::Size(size.width, size.height)));
    }

    fn set_scale(&mut self, scale: f64) {
        CALLS.with_borrow_mut(|calls| {
            // A scale told again and again would never end the call.
            assert!(calls.len() < 100, "set_scale went round");
            calls.push(Call::Scale(scale));
        });
    }

    fn tick(&mut self) {
        CALLS.with_borrow_mut(|calls| calls.push(Call::Tick));
    }
}

#[derive(Clone)]
struct Level {
    gain: Float,
}

impl Params for Level {
    fn declare(p: &mut Declarer) -> Self {
        Level {
            gain: p.add(FloatDecl::new("gain", "Gain", 0.0..=4.0, 2.0)),
        }
    }
}

/// Scales every channel by its gain, linearly.
struct Scale(Level);

impl Processor for Scale {
    type Layout = AnyLayout;

    fn process(&mut self, mut block: Block<'_, AnyLayout>) -> Status {
        let gain = self.0.gain.get();
        block.audio.apply_gain(gain);
        Status::Continue
    }
}

struct Amp {
    params: Level,
}

impl Plugin for Amp {
    const INFO: PluginInfo = PluginInfo {
        name: "Amp",
        vendor: "Test",
        url: "",
        email: "",
        description: "",
        version: "0.1.0",
        id: "dev.test.amp",
        vst3_class_id: [0; 16],
        au_codes: AuCodes {
            subtype: *b"amp ",
            manufacturer: *b"Test",
        },
        category: Category::Effect,
        features: &[],
        layouts: &[Width::STEREO],
        midi_input: false,
        midi_output: false,
    };
    type Params = Level;
    type Processor = Scale;

    fn new(params: Level, _parts: Parts) -> Self {
        Amp { params }
    }

    fn activate(&mut self, _setup: &ProcessSetup) -> Scale {
        Scale(self.params.clone())
    }

    fn editor(&mut self) -> Option<Box<dyn Editor>> {
        HAS_EDITOR.get().then(|| Box::new(Stub) as Box<dyn Editor>)
    }
}

impl Drop for Amp {
    fn drop(&mut self) {
        PLUGINS_DROPPED.set(PLUGINS_DROPPED.get() + 1);
        assert!(!PLUGIN_DROP_PANICS.get(), "the plugin's drop is broken");
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        assert!(!EDITOR_DROP_PANICS.get(), "the editor's drop is broken");
    }
}

/// A window by a number that names none, for a stub editor that never
/// borrows it.
struct NoWindow;

impl HasWindowHandle for NoWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = RawWindowHandle::Win32(Win32WindowHandle::new(NonZeroIsize::MIN));
        // SAFETY: a handle is checked by the system at each use, and the stub
        // editor uses it for nothing.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

#[test]
fn a_block_before_an_activation_is_silence_and_after_it_plays() {
    let app = Standalone::<Amp>::new();
    let mut audio = app.audio();
    let input = [0.5_f32; 8];
    let (mut left, mut right) = ([9.0_f32; 8], [9.0_f32; 8]);
    let played = audio.process(
        &[&input[..], &input[..]],
        &mut [&mut left[..], &mut right[..]],
        8,
        &[],
        |_| {},
    );
    assert_eq!(played, None);
    assert_eq!((left, right), ([0.0; 8], [0.0; 8]), "silence is written");

    app.activate(48_000.0, 4, Width::STEREO).expect("stereo");
    let played = audio.process(
        &[&input[..], &input[..]],
        &mut [&mut left[..], &mut right[..]],
        8,
        &[],
        |_| {},
    );
    assert_eq!(played, Some(Status::Continue));
    assert_eq!((left, right), ([1.0; 8], [1.0; 8]), "in pieces of 4");
    drop(audio);
}

#[test]
fn a_block_is_as_long_as_its_shortest_channel() {
    let app = Standalone::<Amp>::new();
    app.activate(48_000.0, 64, Width::STEREO).expect("stereo");
    let mut audio = app.audio();
    let input = [0.5_f32; 4];
    let (mut left, mut right) = ([9.0_f32; 8], [9.0_f32; 8]);
    audio.process(
        &[&input[..], &input[..]],
        &mut [&mut left[..], &mut right[..]],
        8,
        &[],
        |_| {},
    );
    assert_eq!(left[..4], [1.0; 4]);
    assert_eq!(left[4..], [9.0; 4], "past the shortest, untouched");
    drop(audio);
}

#[test]
fn a_block_of_no_frames_does_not_play_and_its_outputs_are_silence() {
    let app = Standalone::<Amp>::new();
    app.activate(48_000.0, 64, Width::STEREO).expect("stereo");
    let mut audio = app.audio();
    let (input, empty) = ([0.5_f32; 8], [0.0_f32; 0]);
    let (mut left, mut right) = ([9.0_f32; 8], [9.0_f32; 8]);
    let status = audio.process(
        &[&input[..], &empty[..]],
        &mut [&mut left[..], &mut right[..]],
        8,
        &[],
        |_| {},
    );
    assert_eq!(status, None, "no frames to play");
    assert_eq!((left, right), ([0.0; 8], [0.0; 8]), "silence, as None says");
    drop(audio);
}

#[test]
fn channels_past_the_most_are_passed_over() {
    let app = Standalone::<Amp>::new();
    app.activate(48_000.0, 64, Width::STEREO).expect("stereo");
    let mut audio = app.audio();
    let mut channels = vec![[3.0_f32; 4]; MAX_CHANNELS + 1];
    let mut outputs: Vec<&mut [f32]> = channels.iter_mut().map(|c| &mut c[..]).collect();
    audio.process(&[], &mut outputs, 4, &[], |_| {});
    assert_eq!(channels[MAX_CHANNELS], [3.0; 4], "left as it was");
    assert_eq!(
        channels[2], [0.0; 4],
        "a channel the layout lacks is silence"
    );
    drop(audio);
}

#[test]
fn inputs_that_are_one_buffer_play_and_ones_that_share_part_of_one_do_not() {
    let app = Standalone::<Amp>::new();
    app.activate(48_000.0, 64, Width::STEREO).expect("stereo");
    let mut audio = app.audio();
    let input = [0.5_f32; 8];
    let (mut left, mut right) = ([0.0_f32; 4], [0.0_f32; 4]);
    let same = [&input[..4], &input[..4]];
    assert!(
        audio
            .process(&same, &mut [&mut left[..], &mut right[..]], 4, &[], |_| {})
            .is_some()
    );
    assert_eq!(left, [1.0; 4]);
    let shifted = [&input[..4], &input[2..6]];
    assert_eq!(
        audio.process(
            &shifted,
            &mut [&mut left[..], &mut right[..]],
            4,
            &[],
            |_| {}
        ),
        None
    );
    assert_eq!((left, right), ([0.0; 4], [0.0; 4]), "silence");
    let apart = [&input[..4], &input[4..]];
    assert!(
        audio
            .process(&apart, &mut [&mut left[..], &mut right[..]], 4, &[], |_| {})
            .is_some()
    );
    drop(audio);
}

#[test]
fn an_activation_refuses_a_layout_and_a_rate_it_cannot_take() {
    let app = Standalone::<Amp>::new();
    assert_eq!(
        app.activate(48_000.0, 512, Width::MONO),
        Err(Refused::Layout(Width::MONO))
    );
    let refused = app.activate(-1.0, 512, Width::STEREO);
    assert_eq!(refused, Err(Refused::SampleRate(-1.0)));
    assert!(refused.expect_err("refused").to_string().contains("-1"));
}

#[test]
fn the_editor_opens_in_the_apps_window_ticks_with_the_app_and_closes_as_it_drops() {
    HAS_EDITOR.set(true);
    let app = Standalone::<Amp>::new();
    let editor = app.editor().expect("the stub");
    assert_eq!(
        editor.size(),
        Size {
            width: 300,
            height: 200
        }
    );
    editor.set_scale(1.5);
    assert_eq!(editor.scale(), 1.5);
    editor.open(&NoWindow).expect("opens");
    assert!(editor.is_open());
    app.tick();
    editor.set_size(Size {
        width: 400,
        height: 250,
    });
    assert!(matches!(editor.open(&NoWindow), Err(OpenError::Open)));
    drop(editor);
    app.tick();
    assert_eq!(
        calls(),
        [
            Call::Scale(1.5),
            Call::Size(300, 200),
            Call::Open,
            Call::Tick,
            Call::Size(400, 250),
            Call::Close,
        ],
        "told before it opens, and no tick once it closed"
    );
    HAS_EDITOR.set(false);
}

#[test]
fn a_scale_that_is_no_positive_number_is_passed_over() {
    HAS_EDITOR.set(true);
    let app = Standalone::<Amp>::new();
    let editor = app.editor().expect("the stub");
    editor.open(&NoWindow).expect("opens");
    drop(calls());
    for scale in [f64::NAN, f64::INFINITY, 0.0, -1.0] {
        editor.set_scale(scale);
    }
    assert_eq!(editor.scale(), 1.0, "the scale before");
    editor.set_scale(2.0);
    assert_eq!(calls(), [Call::Scale(2.0)]);
    assert_eq!(app.faults(), 0);
    drop(editor);
    HAS_EDITOR.set(false);
}

#[test]
fn an_editor_that_refuses_to_open_says_why() {
    HAS_EDITOR.set(true);
    OPEN_FAILS.set(true);
    let app = Standalone::<Amp>::new();
    let editor = app.editor().expect("the stub");
    let refused = editor.open(&NoWindow).expect_err("refused");
    assert!(refused.to_string().contains("no toolkit here"), "{refused}");
    assert!(!editor.is_open());
    assert_eq!(app.faults(), 1, "a fault of the instance");
    drop(editor);
    calls();
    OPEN_FAILS.set(false);
    HAS_EDITOR.set(false);
}

#[test]
fn the_saved_block_carries_the_parameters_between_runs() {
    let first = Standalone::<Amp>::new();
    first.activate(48_000.0, 64, Width::STEREO).expect("stereo");
    let mut audio = first.audio();
    let input = [1.0_f32; 4];
    let (mut left, mut right) = ([0.0_f32; 4], [0.0_f32; 4]);
    audio.process(
        &[&input[..], &input[..]],
        &mut [&mut left[..], &mut right[..]],
        4,
        &[],
        |_| {},
    );
    drop(audio);
    let saved = first.save();
    drop(first);
    let second = Standalone::<Amp>::new();
    let gain = second.params().id("gain").expect("declared");
    second.params().audio().host_write(gain, 0.75);
    assert_eq!(second.params().value(gain), 3.0);
    second.load(&saved).expect("the saved block");
    assert_eq!(second.params().value(gain), 2.0, "the saved value");
    let refused = second.load(b"not a block").expect_err("refused");
    assert!(
        matches!(refused, LoadRefused::Block(LoadError::NotJson)),
        "{refused}"
    );
    assert_eq!(refused.to_string(), LoadError::NotJson.to_string());
    assert_eq!((second.latency(), second.tail()), (0, Tail::None));
    second.reset();
}

thread_local! {
    /// The value the last [`Trimmed`] saves under `trim`.
    static TRIM: RefCell<Option<Value<f32>>> = const { RefCell::new(None) };
}

/// Keeps its parts, and scales by a saved value it reads as it activates.
struct Trimmed {
    _parts: Parts,
    trim: Value<f32>,
}

/// Scales every channel by the trim its activation found.
struct Fixed(f32);

impl Processor for Fixed {
    type Layout = AnyLayout;

    fn process(&mut self, mut block: Block<'_, AnyLayout>) -> Status {
        block.audio.apply_gain(self.0);
        Status::Continue
    }
}

impl Plugin for Trimmed {
    const INFO: PluginInfo = Amp::INFO;
    type Params = Level;
    type Processor = Fixed;

    fn new(_params: Level, mut parts: Parts) -> Self {
        let trim = parts.saved_value("trim", 1.0_f32).expect("a free key");
        TRIM.set(Some(trim.clone()));
        Trimmed {
            _parts: parts,
            trim,
        }
    }

    fn activate(&mut self, _setup: &ProcessSetup) -> Fixed {
        Fixed(self.trim.get())
    }
}

#[test]
fn a_load_before_an_activation_plays_in_the_first_block() {
    let first = Standalone::<Trimmed>::new();
    TRIM.with_borrow(|trim| trim.as_ref().expect("made").set(0.25));
    first.tick();
    let saved = first.save();
    drop(first);
    // As the app starts: the saved block, then the audio, and no tick yet.
    let second = Standalone::<Trimmed>::new();
    second.load(&saved).expect("the saved block");
    second
        .activate(48_000.0, 64, Width::STEREO)
        .expect("stereo");
    let mut audio = second.audio();
    let input = [1.0_f32; 4];
    let (mut left, mut right) = ([0.0_f32; 4], [0.0_f32; 4]);
    audio.process(
        &[&input[..], &input[..]],
        &mut [&mut left[..], &mut right[..]],
        4,
        &[],
        |_| {},
    );
    assert_eq!((left, right), ([0.25; 4], [0.25; 4]));
    assert_eq!(second.faults(), 0);
}

/// The amp at its state's version 1, with a migration and an activation that
/// panic.
struct Migrating;

impl Plugin for Migrating {
    const INFO: PluginInfo = Amp::INFO;
    const SCHEMA_VERSION: u64 = 1;
    type Params = Level;
    type Processor = Scale;

    fn new(_params: Level, _parts: Parts) -> Self {
        Migrating
    }

    fn migrate(
        _from: u64,
        _block: &mut serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        panic!("the migration is broken");
    }

    fn activate(&mut self, _setup: &ProcessSetup) -> Scale {
        panic!("the activation is broken");
    }
}

#[test]
fn a_migration_that_panics_refuses_the_load_and_counts_a_fault() {
    let app = Standalone::<Migrating>::new();
    let older = br#"{"haptoStateVersion": 1, "pluginStateVersion": 0, "parameters": {}}"#;
    let refused = app.load(older).expect_err("refused, as a host's load is");
    assert!(matches!(refused, LoadRefused::Panicked), "{refused}");
    assert_eq!(app.faults(), 1);
    assert!(!app.save().is_empty(), "the app goes on");
}

#[test]
fn an_activation_that_panics_is_refused_as_a_panic() {
    let app = Standalone::<Migrating>::new();
    let refused = app.activate(48_000.0, 64, Width::STEREO);
    assert_eq!(
        refused,
        Err(Refused::Panicked),
        "not taken for a busy plugin"
    );
    assert_eq!(app.faults(), 1);
    assert!(
        refused
            .expect_err("refused")
            .to_string()
            .contains("panicked")
    );
}

#[test]
fn a_teardown_frees_the_plugin_only_once_no_audio_side_is_left() {
    let before = PLUGINS_DROPPED.get();
    let faults = thread_faults();
    let app = Standalone::<Amp>::new();
    drop(app.audio());
    drop(app);
    assert_eq!(PLUGINS_DROPPED.get(), before + 1, "freed");
    assert_eq!(thread_faults(), faults, "a teardown in order");

    let app = Standalone::<Amp>::new();
    let audio = app.audio();
    written();
    drop(app);
    drop(audio);
    assert_eq!(PLUGINS_DROPPED.get(), before + 1, "leaked, not freed here");
    assert_eq!(thread_faults(), faults + 1, "a fault of the app's");
    let lines = written();
    assert!(
        lines.iter().any(|line| line.contains("audio side")),
        "{lines:?}"
    );
}

#[test]
fn an_editor_may_outlive_the_app_and_frees_the_plugin_as_it_drops() {
    HAS_EDITOR.set(true);
    let before = PLUGINS_DROPPED.get();
    let faults = thread_faults();
    let app = Standalone::<Amp>::new();
    let editor = app.editor().expect("the stub");
    editor.open(&NoWindow).expect("opens");
    drop(app);
    assert_eq!(PLUGINS_DROPPED.get(), before, "the editor holds it");
    drop(editor);
    assert_eq!(PLUGINS_DROPPED.get(), before + 1, "freed here, not leaked");
    assert_eq!(thread_faults(), faults, "no fault");
    assert_eq!(calls().last(), Some(&Call::Close), "closed as it dropped");
    HAS_EDITOR.set(false);
}

#[test]
fn an_editor_that_panics_as_it_drops_is_a_fault_and_the_app_goes_on() {
    HAS_EDITOR.set(true);
    let app = Standalone::<Amp>::new();
    let editor = app.editor().expect("the stub");
    editor.open(&NoWindow).expect("opens");
    EDITOR_DROP_PANICS.set(true);
    drop(editor);
    EDITOR_DROP_PANICS.set(false);
    assert_eq!(app.faults(), 1, "a fault of the instance");
    calls();
    HAS_EDITOR.set(false);
}

#[test]
fn a_plugin_that_panics_as_it_drops_is_a_fault_and_the_app_goes_on() {
    let before = PLUGINS_DROPPED.get();
    let faults = thread_faults();
    let app = Standalone::<Amp>::new();
    PLUGIN_DROP_PANICS.set(true);
    drop(app);
    PLUGIN_DROP_PANICS.set(false);
    assert_eq!(PLUGINS_DROPPED.get(), before + 1, "freed");
    assert_eq!(thread_faults(), faults + 1, "counted for the process");
}

#[test]
fn a_panic_while_the_editor_opens_is_refused_as_one() {
    HAS_EDITOR.set(true);
    REASON_PANICS.set(true);
    let app = Standalone::<Amp>::new();
    let editor = app.editor().expect("the stub");
    let refused = editor.open(&NoWindow).expect_err("refused");
    REASON_PANICS.set(false);
    assert!(matches!(refused, OpenError::Panicked), "{refused}");
    assert!(app.faults() >= 1);
    drop(editor);
    calls();
    HAS_EDITOR.set(false);
}

#[test]
fn an_open_refused_while_the_editor_is_still_in_a_call_is_a_fault() {
    HAS_EDITOR.set(true);
    let app = Standalone::<Amp>::new();
    let editor = std::rc::Rc::new(app.editor().expect("the stub"));
    let inner = std::rc::Rc::clone(&editor);
    let busy = std::rc::Rc::new(Cell::new(false));
    let seen = std::rc::Rc::clone(&busy);
    DURING_OPEN.with_borrow_mut(|during| {
        *during = Some(Box::new(move || {
            // A close and an open again from inside the first open.
            inner.close();
            seen.set(matches!(inner.open(&NoWindow), Err(OpenError::Busy)));
        }));
    });
    let _first = editor.open(&NoWindow);
    assert!(
        busy.get(),
        "the second open, from inside the first, is busy"
    );
    assert_eq!(
        app.faults(),
        1,
        "a fault of the instance, as the VST3 view counts it"
    );
    drop(editor);
    calls();
    HAS_EDITOR.set(false);
}

#[test]
fn a_teardown_while_the_audio_thread_plays_leaks_the_engine_and_silences_it() {
    use std::sync::Arc as Shared;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    let faults = thread_faults();
    let app = Standalone::<Amp>::new();
    app.activate(48_000.0, 64, Width::STEREO).expect("stereo");
    let mut audio = app.audio();
    // The engine, watched: were the audio side's drop to free it, it would
    // be freed on the audio thread.
    let engine = Shared::downgrade(&audio.engine);
    let played = Shared::new(AtomicUsize::new(0));
    let silenced = Shared::new(AtomicBool::new(false));
    let stop = Shared::new(AtomicBool::new(false));
    let thread = {
        let (played, silenced, stop) = (played.clone(), silenced.clone(), stop.clone());
        std::thread::spawn(move || {
            let input = [0.5_f32; 64];
            let (mut left, mut right) = ([0.0_f32; 64], [0.0_f32; 64]);
            while !stop.load(Ordering::Acquire) {
                let inputs = [&input[..], &input[..]];
                match audio.process(
                    &inputs,
                    &mut [&mut left[..], &mut right[..]],
                    64,
                    &[],
                    |_| {},
                ) {
                    Some(_) => drop(played.fetch_add(1, Ordering::AcqRel)),
                    None => silenced.store(true, Ordering::Release),
                }
                std::thread::yield_now();
            }
            // The audio side goes here, on its own thread, after the app.
            drop(audio);
        })
    };
    let wait = |what: &str, until: &dyn Fn() -> bool| {
        let end = Instant::now() + Duration::from_secs(20);
        while !until() {
            assert!(Instant::now() < end, "{what}");
            std::thread::yield_now();
        }
    };
    wait("the audio thread plays", &|| {
        played.load(Ordering::Acquire) >= 3
    });
    // The app goes while blocks run on the other thread.
    drop(app);
    wait("the blocks after it play silence", &|| {
        silenced.load(Ordering::Acquire)
    });
    stop.store(true, Ordering::Release);
    thread.join().expect("the audio thread ended");
    assert_eq!(
        thread_faults(),
        faults + 1,
        "an audio side outlived the app"
    );
    assert!(
        engine.upgrade().is_some(),
        "leaked, not freed as the audio side dropped on its thread"
    );
}

/// Writes its MIDI into its audio, so a test reads what reached it without
/// a lock: on the left, how many packets each frame carries, and on the
/// right, how many of them were upgraded from MIDI 1.0.
struct Counts;

impl Processor for Counts {
    type Layout = Stereo;

    fn process(&mut self, mut block: Block<'_, Stereo>) -> Status {
        block.audio.clear();
        let [at, upgraded] = block.audio.channels_mut();
        for event in block.midi.packets() {
            let frame = event.offset as usize;
            at[frame] += 1.0;
            if event.origin == hapto_ump::Origin::Midi1 {
                upgraded[frame] += 1.0;
            }
        }
        Status::Continue
    }
}

struct Listener;

impl Plugin for Listener {
    const INFO: PluginInfo = PluginInfo {
        name: "Listener",
        id: "dev.test.listener",
        layouts: &[],
        midi_input: true,
        ..Amp::INFO
    };
    type Params = Level;
    type Processor = Counts;

    fn new(_params: Level, _parts: Parts) -> Self {
        Listener
    }

    fn activate(&mut self, _setup: &ProcessSetup) -> Counts {
        Counts
    }
}

/// Sends on every message it reads, at its frame: MIDI thru.
struct Forwarding;

impl Processor for Forwarding {
    type Layout = Stereo;

    fn process(&mut self, block: Block<'_, Stereo>) -> Status {
        let Block {
            midi, mut midi_out, ..
        } = block;
        midi_out.extend(midi);
        Status::Continue
    }
}

struct Thru;

impl Plugin for Thru {
    const INFO: PluginInfo = PluginInfo {
        name: "Thru",
        id: "dev.test.thru",
        layouts: &[],
        midi_input: true,
        midi_output: true,
        ..Amp::INFO
    };
    type Params = Level;
    type Processor = Forwarding;

    fn new(_params: Level, _parts: Parts) -> Self {
        Thru
    }

    fn activate(&mut self, _setup: &ProcessSetup) -> Forwarding {
        Forwarding
    }
}

#[test]
fn what_the_plugin_sends_comes_back_at_its_frames_of_the_block_once_it_has_played() {
    let app = Standalone::<Thru>::new();
    let mut audio = app.audio();
    let note = Event::new(6, [0x4090_3C00, 0xFFFF_0000, 0, 0]);
    let volume = Event::new(2, [0x40B0_0700, u32::MAX, 0, 0]);
    let (mut left, mut right) = ([0.0_f32; 8], [0.0_f32; 8]);
    let mut sent = Vec::new();
    // Before an activation nothing plays, so nothing comes back.
    audio.process(
        &[],
        &mut [&mut left[..], &mut right[..]],
        8,
        &[note],
        |packets| sent.extend_from_slice(packets),
    );
    assert_eq!(sent, []);
    // Blocks of at most 4: the note-on is sent by the second piece.
    assert_eq!(app.activate(48_000.0, 4, Width::STEREO), Ok(()));
    audio.process(
        &[],
        &mut [&mut left[..], &mut right[..]],
        8,
        &[volume, note],
        |packets| sent.extend_from_slice(packets),
    );
    let sent: Vec<(u32, u32)> = sent
        .iter()
        .map(|event| (event.offset, event.words[0]))
        .collect();
    assert_eq!(sent, [(2, 0x40B0_0700), (6, 0x4090_3C00)]);
}

/// MIDI thru with no audio at all, as an arpeggiator is.
struct Bare;

impl Processor for Bare {
    type Layout = NoAudio;

    fn process(&mut self, block: Block<'_, NoAudio>) -> Status {
        let Block {
            midi, mut midi_out, ..
        } = block;
        midi_out.extend(midi);
        Status::Continue
    }
}

struct BareThru;

impl Plugin for BareThru {
    const INFO: PluginInfo = PluginInfo {
        name: "Bare thru",
        id: "dev.test.bare-thru",
        layouts: &[],
        midi_input: true,
        midi_output: true,
        ..Amp::INFO
    };
    type Params = Level;
    type Processor = Bare;

    fn new(_params: Level, _parts: Parts) -> Self {
        BareThru
    }

    fn activate(&mut self, _setup: &ProcessSetup) -> Bare {
        Bare
    }
}

#[test]
fn a_plugin_with_no_channels_plays_its_midi_for_the_frames_it_is_given() {
    let app = Standalone::<BareThru>::new();
    assert_eq!(app.activate(48_000.0, 64, Width::new(0, 0)), Ok(()));
    let mut audio = app.audio();
    let note = Event::new(6, [0x4090_3C00, 0xFFFF_0000, 0, 0]);
    let mut sent = Vec::new();
    let played = audio.process(&[], &mut [], 8, &[note], |packets| {
        sent.extend_from_slice(packets);
    });
    assert_eq!(played, Some(Status::Continue));
    let sent: Vec<(u32, u32)> = sent
        .iter()
        .map(|event| (event.offset, event.words[0]))
        .collect();
    assert_eq!(sent, [(6, 0x4090_3C00)]);
    // A block of no frames plays nothing, channels or not.
    assert_eq!(audio.process(&[], &mut [], 0, &[note], |_| {}), None);
}

#[test]
fn a_panic_in_what_is_handed_the_plugins_midi_is_counted_and_the_plugin_plays_on() {
    let app = Standalone::<Thru>::new();
    assert_eq!(app.activate(48_000.0, 8, Width::STEREO), Ok(()));
    let mut audio = app.audio();
    let note = Event::new(2, [0x4090_3C00, 0xFFFF_0000, 0, 0]);
    let (mut left, mut right) = ([0.0_f32; 8], [0.0_f32; 8]);
    let mut block = |sent: &mut dyn FnMut(&[Event])| {
        audio.process(
            &[],
            &mut [&mut left[..], &mut right[..]],
            8,
            &[note],
            |packets| sent(packets),
        )
    };
    let played = block(&mut |_| panic!("the app's own mistake"));
    assert_eq!(played, Some(Status::Continue), "the block had played");
    assert_eq!(app.faults(), 1, "counted, as the exporter's");
    let mut sent = 0;
    let played = block(&mut |packets| sent += packets.len());
    assert_eq!(
        played,
        Some(Status::Continue),
        "the processor was not faulted"
    );
    assert_eq!(sent, 1);
}

#[test]
fn the_apps_midi_reaches_the_processor_at_its_frames_and_midi_1_upgraded() {
    use hapto_ump::{Midi1Message, Midi1Voice};

    let app = Standalone::<Listener>::new();
    assert_eq!(app.activate(48_000.0, 64, Width::STEREO), Ok(()));
    let mut audio = app.audio();
    let message = Midi1Message::ControlChange {
        controller: 7,
        value: 100,
    };
    let midi1 = Midi1Voice {
        group: 0,
        channel: 0,
        message,
    }
    .event(2);
    let midi2 = Event::new(5, [0x40B0_0700, 1, 0, 0]);
    // Past the block's end, so at its last frame.
    let late = Event {
        offset: 50,
        ..midi1
    };
    let (mut left, mut right) = ([0.0_f32; 8], [0.0_f32; 8]);
    let played = audio.process(
        &[],
        &mut [&mut left[..], &mut right[..]],
        8,
        &[midi1, midi2, late],
        |_| {},
    );
    assert_eq!(played, Some(Status::Continue));
    assert_eq!(left, [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0]);
    assert_eq!(
        right,
        [0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        "MIDI 2.0 as sent"
    );
}
