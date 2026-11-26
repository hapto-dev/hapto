//! Tests that the builder's settings reach the MIDI devices' handle, and
//! that the app fails with a reason where it doesn't run.

use hapto_host::layout::Stereo;
use hapto_host::{AuCodes, Block, Category, Parts, PluginInfo, ProcessSetup, Processor, Status};
use hapto_params::{Declarer, Params};

use super::*;

#[derive(Clone)]
struct NoParams;

impl Params for NoParams {
    fn declare(_: &mut Declarer) -> Self {
        NoParams
    }
}

struct Pass;

impl Processor for Pass {
    type Layout = Stereo;

    fn process(&mut self, _: Block<'_, Stereo>) -> Status {
        Status::Continue
    }
}

/// A plugin that does nothing, for the app to run.
struct Quiet;

impl Plugin for Quiet {
    const INFO: PluginInfo = PluginInfo {
        name: "Quiet",
        vendor: "Hapto",
        url: "",
        email: "",
        description: "",
        version: "0.1.0",
        id: "dev.hapto.test.quiet",
        vst3_class_id: [0; 16],
        au_codes: AuCodes {
            subtype: *b"quie",
            manufacturer: *b"Hapt",
        },
        category: Category::Effect,
        features: &[],
        layouts: &[],
        midi_input: true,
        midi_output: true,
    };
    type Params = NoParams;
    type Processor = Pass;

    fn new(_: NoParams, _: Parts) -> Self {
        Quiet
    }

    fn activate(&mut self, _: &ProcessSetup) -> Pass {
        Pass
    }
}

#[test]
fn what_the_builder_says_reaches_the_handle_the_app_hands_out() {
    let app = App::<Quiet>::new()
        .with_midi_inputs(|device| device.name() == "Keys")
        .with_midi_outputs(|_| true)
        .with_midi_out_timing(Timing::AtFrames);
    let midi = app.midi();
    assert_eq!(midi.timing(), Timing::AtFrames);
    let (changes, inputs, outputs) = midi.filters();
    assert_eq!(changes, 3, "one change for each call");
    assert!(inputs(&Device::for_tests("Keys", Direction::Input)));
    assert!(!inputs(&Device::for_tests("Pads", Direction::Input)));
    assert!(outputs(&Device::for_tests("Synth", Direction::Output)));
    // A default app opens every input and no output, and names its plugin.
    let plain = App::<Quiet>::default();
    let (changes, inputs, outputs) = plain.midi().filters();
    assert_eq!(changes, 0);
    assert!(inputs(&Device::for_tests("Pads", Direction::Input)));
    assert!(!outputs(&Device::for_tests("Synth", Direction::Output)));
    assert!(format!("{plain:?}").contains("Quiet"));
}

#[cfg(not(hapto_app))]
#[test]
fn where_the_app_does_not_run_it_fails_and_says_why() {
    assert_eq!(App::<Quiet>::new().run(), ExitCode::FAILURE);
    assert_eq!(main::<Quiet>(), ExitCode::FAILURE);
}

#[test]
fn the_dependency_table_names_the_systems_that_the_build_script_lists() {
    hapto_testkit::systems::assert_target_tables_match(
        include_str!("../Cargo.toml"),
        env!("HAPTO_APP_SYSTEMS"),
    );
}
