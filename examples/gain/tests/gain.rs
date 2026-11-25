//! The plugin through the VST3 test host where a host can reach it (audio,
//! parameters, the saved block), and driven directly where only the plugin's
//! own code can (the label, undo, MIDI learn).

use std::sync::Arc;

use hapto_example_gain::Gain;
use hapto_host::layout::Width;
use hapto_host::midi::MidiList;
use hapto_host::vst3::TestHost;
use hapto_host::{
    AuCodes, Audio, Block, Category, Driver, EventList, MidiOut, Parts, Plugin, PluginInfo,
    ProcessSetup, Processor, SavedState,
};
use hapto_params::host_id;
use hapto_ump::{Midi1Message, Midi1Voice, Upgrade};

const GAIN: &str = "gain";
const MODE: &str = "mode";
const BYPASS: &str = "bypass";

/// The plugin, built as an exporter builds it, the driver that ticks its
/// parts as the exporter's engine would, and the saved state the host's save
/// and load would go through.
fn built() -> (Gain, Driver, Arc<SavedState>) {
    let (params, parts) = Parts::for_plugin::<Gain>().expect("valid declarations");
    let driver = Driver::of(&parts);
    let saved = Arc::clone(parts.saved());
    (Gain::new(params, parts), driver, saved)
}

fn running() -> TestHost<Gain> {
    let host = TestHost::<Gain>::new();
    assert_eq!(host.setup(48_000.0, 64), Ok(()));
    assert_eq!(host.activate(), Ok(()));
    assert_eq!(host.start(), Ok(()));
    host
}

#[test]
fn the_manifest_gives_the_identity_hosts_know_the_gain_by() {
    // A host takes a plugin whose ids change for another plugin, and its
    // sessions lose this one.
    let expected = PluginInfo {
        name: "Hapto Example Gain",
        vendor: "Hapto",
        url: "",
        email: "",
        description: "A gain with three modes, a label and MIDI learn",
        version: env!("CARGO_PKG_VERSION"),
        id: "dev.hapto.example-gain",
        vst3_class_id: 0x5b0a_d271_3e96_4c18_a37f_e024_8db5_6a19_u128.to_be_bytes(),
        au_codes: AuCodes {
            subtype: *b"SmGn",
            manufacturer: *b"Hapt",
        },
        category: Category::Effect,
        features: &[],
        // Its processor is in `Stereo`, which runs at its own width.
        layouts: &[],
        midi_input: true,
        midi_output: false,
    };
    assert_eq!(Gain::INFO, expected);
}

#[test]
fn the_gain_applies_through_the_exporter() {
    let mut host = running();
    let gain = host_id(GAIN);
    let plus_six = host.normalize(gain, 6.0);
    let (mut left, mut right) = ([0.5_f32; 8], [-0.25_f32; 8]);
    assert_eq!(
        host.process_in_place(&mut [&mut left, &mut right], 8, &[(gain, plus_six)]),
        Ok(())
    );
    let factor = 10.0_f32.powf(6.0 / 20.0);
    assert!((left[7] - 0.5 * factor).abs() < 1e-6, "{}", left[7]);
    assert!((right[0] + 0.25 * factor).abs() < 1e-6, "{}", right[0]);
}

#[test]
fn bypass_passes_the_input_through() {
    let mut host = running();
    let changes = [
        (host_id(GAIN), host.normalize(host_id(GAIN), 12.0)),
        (host_id(MODE), 1.0),
        (host_id(BYPASS), 1.0),
    ];
    let (mut left, mut right) = ([0.5_f32; 8], [-0.25_f32; 8]);
    assert_eq!(
        host.process_in_place(&mut [&mut left, &mut right], 8, &changes),
        Ok(())
    );
    assert_eq!((left, right), ([0.5; 8], [-0.25; 8]));
}

#[test]
fn the_host_sees_three_named_modes_and_one_bypass() {
    let host = TestHost::<Gain>::new();
    let mode = host.parameter(1).expect("listed");
    assert_eq!(mode.id, host_id(MODE));
    assert!(mode.is_list);
    assert_eq!(mode.step_count, 2);
    let names: Vec<_> = [0.0, 0.5, 1.0]
        .iter()
        .map(|&normalized| host.text(mode.id, normalized).expect("a name"))
        .collect();
    assert_eq!(names, ["Clean", "Warm", "Hot"]);
    let bypass = host.parameter(2).expect("listed");
    assert!(bypass.is_bypass && !mode.is_bypass);
}

#[test]
fn the_label_saves_and_loads_back() {
    let (plugin, driver, saved) = built();
    plugin.set_label("Drum bus");
    // The label is captured for the save at the next tick.
    driver.tick();
    let block = saved.save();

    // Through the exporter, as a host reopening the project.
    let host = TestHost::<Gain>::new();
    assert_eq!(host.load(&block), Ok(()));
    assert_eq!(host.save().as_ref(), Ok(&block));

    // Into a new instance: the label applies on its next tick.
    let (reopened, driver, saved) = built();
    saved.load(&block).expect("a saved block");
    assert_eq!(reopened.label(), "Untitled");
    driver.tick();
    assert_eq!(reopened.label(), "Drum bus");
    assert_eq!(
        reopened.history().undo(),
        Ok(false),
        "a load leaves nothing to undo"
    );
}

#[test]
fn undo_and_redo_restore_the_label_and_its_saved_state() {
    let (plugin, driver, saved) = built();
    plugin.set_label("Vocals");
    driver.tick();
    plugin.set_label("Lead vocals");

    assert_eq!(plugin.history().undo(), Ok(true));
    assert_eq!(plugin.label(), "Vocals");
    driver.tick();
    let saved = String::from_utf8(saved.save()).expect("UTF-8");
    assert!(saved.contains(r#""label":"Vocals""#), "{saved}");

    assert_eq!(plugin.history().undo(), Ok(true));
    assert_eq!(plugin.label(), "Untitled");
    assert_eq!(plugin.history().redo(), Ok(true));
    assert_eq!(plugin.label(), "Vocals");
}

#[test]
fn two_lone_writes_are_two_steps_and_the_labels_sets_between_ticks_are_one() {
    let (plugin, _, _) = built();
    let gain = plugin.ui().set().id(GAIN).expect("declared");

    // Each lone write is a gesture of its own, so each is a step. Undo
    // records what changed since the last tick before it undoes.
    assert!(plugin.ui().write(gain, -6.0));
    assert!(plugin.ui().write(gain, -12.0));
    assert_eq!(plugin.history().undo(), Ok(true));
    assert_eq!(plugin.ui().value(gain), -6.0);

    // The label's sets between two ticks are one step, the latest winning.
    plugin.set_label("Vocals");
    plugin.set_label("Lead vocals");
    assert_eq!(plugin.history().undo(), Ok(true));
    assert_eq!(plugin.label(), "Untitled");
    assert_eq!(plugin.history().undo(), Ok(true));
    assert_eq!(plugin.ui().value(gain), 0.0);
}

#[test]
fn a_learned_controller_drives_the_gain() {
    let (mut plugin, driver, saved) = built();
    let mut setup = ProcessSetup::new(48_000.0, 8, Width::STEREO);
    setup.max_events = 4;
    let mut processor = plugin.activate(&setup);
    // As an activation takes it, and as each block runs it.
    let mut learn = driver.learn_audio().expect("learn is on");
    let cc = |controller, value| {
        let mut events = EventList::with_capacity(1);
        let message = Midi1Message::ControlChange { controller, value };
        let sent = Midi1Voice {
            group: 0,
            channel: 0,
            message,
        }
        .event(0);
        Upgrade::new().block(&[sent], &mut events);
        events
    };
    let mut midi = MidiList::new();
    let mut run = |processor: &mut _, events: &EventList| {
        learn.process(events);
        midi.read(events, 8);
        let (mut left, mut right) = ([0.5_f32; 8], [0.5_f32; 8]);
        let mut out = EventList::with_capacity(0);
        let mut channels = [&mut left[..], &mut right[..]];
        Processor::process(
            processor,
            Block::new(
                Audio::new(&mut channels, 8),
                midi.midi(),
                MidiOut::new(&mut out),
            ),
        );
        left[0]
    };

    plugin.learn_gain().expect("armed");
    run(&mut processor, &cc(7, 64));
    driver.tick(); // The tick commits what the block staged.
    assert!(plugin.gain_is_learned());

    // Controller 7 at 0 is the bottom of the range, -60 dB, in its own block.
    let level = run(&mut processor, &cc(7, 0));
    assert!((level - 0.0005).abs() < 1e-6, "{level}");

    // The mapping is saved with the plugin.
    let block = saved.save();
    let (reopened, driver, saved) = built();
    saved.load(&block).expect("a saved block");
    driver.tick();
    assert!(reopened.gain_is_learned());
    plugin.deactivate(processor);
}

#[test]
fn the_page_s_learn_button_arms_learn_at_the_next_idle() {
    let (mut plugin, driver, _saved) = built();
    let mut setup = ProcessSetup::new(48_000.0, 8, Width::STEREO);
    setup.max_events = 4;
    let processor = plugin.activate(&setup);
    let mut learn = driver.learn_audio().expect("learn is on");
    let tick = |plugin: &mut Gain| {
        driver.tick();
        plugin.idle();
    };
    tick(&mut plugin);
    assert_eq!(plugin.learn_state(), "off");

    // The page's button requests learn, and the next idle arms it.
    plugin.ask_to_learn();
    tick(&mut plugin);
    assert_eq!(plugin.learn_state(), "armed");

    // A controller in a block, then the tick that commits it.
    let mut events = EventList::with_capacity(1);
    let message = Midi1Message::ControlChange {
        controller: 7,
        value: 64,
    };
    let sent = Midi1Voice {
        group: 0,
        channel: 0,
        message,
    }
    .event(0);
    Upgrade::new().block(&[sent], &mut events);
    learn.process(&events);
    tick(&mut plugin);
    assert_eq!(plugin.learn_state(), "learned");
    assert!(plugin.gain_is_learned());
    plugin.deactivate(processor);
}
