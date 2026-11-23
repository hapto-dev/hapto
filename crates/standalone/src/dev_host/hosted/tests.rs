//! In each format, the dev host changes the audio as a host does: stopped,
//! deactivated, set up again and activated, the plugin builds a processor
//! for the new rate and maximum; and blocks of no frames and of one past the
//! maximum play with no fault.

use std::cell::RefCell;

use hapto_host::layout::{AnyLayout, Width};
use hapto_host::{
    AuCodes, Block, Category, Parts, Plugin, PluginInfo, ProcessSetup, Processor, Status,
};
use hapto_params::{Declarer, Kind, ParamDecl, Params, host_id};

use super::host;
use crate::block::Render;
use crate::dev_host::format::Format;

thread_local! {
    /// Each activation's sample rate and most frames, on the thread that
    /// activates.
    static ACTIVATIONS: RefCell<Vec<(f64, usize)>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone)]
struct NoParams;

impl Params for NoParams {
    fn declare(_: &mut Declarer) -> Self {
        NoParams
    }
}

/// Plays its input through.
struct Through;

impl Processor for Through {
    type Layout = AnyLayout;

    fn process(&mut self, _block: Block<'_, AnyLayout>) -> Status {
        Status::Continue
    }
}

/// A plugin that records each activation.
struct Probe;

impl Plugin for Probe {
    const INFO: PluginInfo = PluginInfo {
        name: "Probe",
        vendor: "Test",
        url: "",
        email: "",
        description: "",
        version: "0.1.0",
        id: "dev.test.probe",
        vst3_class_id: [0; 16],
        au_codes: AuCodes {
            subtype: *b"prob",
            manufacturer: *b"Test",
        },
        category: Category::Effect,
        features: &[],
        layouts: &[Width::STEREO],
        midi_input: false,
        midi_output: false,
    };
    type Params = NoParams;
    type Processor = Through;

    fn new(_params: NoParams, _parts: Parts) -> Self {
        Probe
    }

    fn activate(&mut self, setup: &ProcessSetup) -> Through {
        ACTIVATIONS.with_borrow_mut(|a| a.push((setup.sample_rate, setup.max_frames)));
        Through
    }
}

/// A parameter of each kind.
const KNOBS: &[ParamDecl] = &[
    ParamDecl::new(
        "gain",
        "Gain",
        Kind::continuous(-24.0, 24.0, 0.1, None),
        0.0,
    )
    .with_unit("dB"),
    ParamDecl::new("mode", "Mode", Kind::Choice(&["A", "B", "C"]), 0.0),
    ParamDecl::new("on", "On", Kind::Bool, 1.0),
    ParamDecl::new("bypass", "Bypass", Kind::Bool, 0.0).as_bypass(),
];

#[derive(Clone)]
struct Table;

impl Params for Table {
    fn declare(p: &mut Declarer) -> Self {
        p.extend(KNOBS.iter().copied());
        Table
    }
}

/// A plugin with a parameter of each kind.
struct Knobs;

impl Plugin for Knobs {
    const INFO: PluginInfo = Probe::INFO;
    type Params = Table;
    type Processor = Through;

    fn new(_params: Table, _parts: Parts) -> Self {
        Knobs
    }

    fn activate(&mut self, _setup: &ProcessSetup) -> Through {
        Through
    }
}

/// Plays one stereo block of `frames` frames through `render`.
fn play(render: &mut dyn Render, frames: usize) {
    let input = vec![0.5_f32; frames];
    let (mut left, mut right) = (vec![0.0_f32; frames], vec![0.0_f32; frames]);
    render.render(
        &[&input, &input],
        &mut [&mut left, &mut right],
        &[],
        &mut |_| {},
    );
}

#[test]
fn a_change_of_rate_and_maximum_reactivates_the_plugin_in_each_format() {
    for format in Format::ALL {
        ACTIVATIONS.with_borrow_mut(Vec::clear);
        let hosted = host::<Probe>(format);
        let mut render = hosted.start(48_000.0, 512, Width::STEREO).expect("started");
        play(&mut *render, 512);
        // As the dev host changes a setting: the device's blocks end, then
        // the plugin stops.
        drop(render);
        hosted.stop();
        let mut render = hosted
            .start(96_000.0, 256, Width::STEREO)
            .expect("started again");
        play(&mut *render, 256);
        drop(render);
        let activations = ACTIVATIONS.with_borrow(Clone::clone);
        assert_eq!(
            activations,
            [(48_000.0, 512), (96_000.0, 256)],
            "{format:?}"
        );
        assert_eq!(hosted.faults(), 0, "{format:?}");
    }
}

#[test]
fn each_format_lists_the_parameters_by_kind_with_hapto_s_host_ids() {
    for format in Format::ALL {
        let hosted = host::<Knobs>(format);
        let params = hosted.params();
        let kinds: Vec<&str> = params.iter().map(|p| p.kind.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "continuous, dB",
                "choice of 3",
                "off or on",
                "the host's bypass"
            ],
            "{format:?}"
        );
        let steps: Vec<u32> = params.iter().map(|p| p.steps).collect();
        assert_eq!(steps, [0, 2, 1, 1], "{format:?}: values but one");
        for (param, decl) in params.iter().zip(KNOBS) {
            assert_eq!(param.host_id, host_id(decl.key), "{format:?}");
        }
    }
    let cabi = host::<Knobs>(Format::Cabi);
    assert_eq!(cabi.params()[0].handle, "gain", "the C interface's key");
    let vst3 = host::<Knobs>(Format::Vst3);
    let id = host_id("gain");
    assert_eq!(vst3.params()[0].handle, format!("id {id:#010x}"));
}

#[test]
fn a_control_moved_while_processing_is_stopped_lands_in_each_format() {
    for format in Format::ALL {
        let hosted = host::<Knobs>(format);
        drop(hosted.start(48_000.0, 64, Width::STEREO).expect("started"));
        hosted.stop();
        // No block runs to carry the change, as a host's own control sends
        // one while it processes.
        hosted.set(0, 0.25);
        assert_eq!(hosted.value(0).0, 0.25, "{format:?}");
        assert_eq!(hosted.faults(), 0, "{format:?}");
    }
}

#[test]
fn a_block_that_is_not_a_saved_one_is_refused_in_each_format() {
    for format in Format::ALL {
        let hosted = host::<Knobs>(format);
        let refused = hosted.load(b"not a block").expect_err("refused");
        assert!(
            format!("{refused:#}").contains("answered"),
            "{format:?}: {refused:#}"
        );
        let saved = hosted.save().expect("saved");
        assert!(hosted.load(&saved).is_ok(), "{format:?}");
        assert_eq!(hosted.faults(), 0, "{format:?}");
    }
}

#[test]
fn a_plugin_stopped_twice_starts_again_with_no_fault_in_each_format() {
    for format in Format::ALL {
        let hosted = host::<Probe>(format);
        drop(hosted.start(48_000.0, 64, Width::STEREO).expect("started"));
        // Stopped by the author, then reactivated, which stops it first.
        hosted.stop();
        hosted.stop();
        let mut render = hosted
            .start(48_000.0, 64, Width::STEREO)
            .expect("started again");
        play(&mut *render, 64);
        drop(render);
        assert_eq!(hosted.faults(), 0, "{format:?}");
    }
}

#[test]
fn awkward_blocks_play_with_no_fault_in_each_format() {
    for format in Format::ALL {
        let hosted = host::<Probe>(format);
        let mut render = hosted.start(48_000.0, 64, Width::STEREO).expect("started");
        for frames in [1, 0, 64, 65, 3, 0] {
            play(&mut *render, frames);
        }
        drop(render);
        assert_eq!(hosted.faults(), 0, "{format:?}");
    }
}
