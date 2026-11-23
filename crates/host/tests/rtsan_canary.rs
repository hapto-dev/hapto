//! The real-time sanitizer's canary: a processor that allocates, played
//! through the C interface, whose `process` carries the nonblocking mark.
//!
//! It is built only for the nightly sanitizer lane (`--cfg hapto_rtsan`),
//! which expects the test to abort. A canary that passes shows the lane
//! checks nothing: the marks are missing, or the sanitizer is not linked.
#![cfg(all(hapto_rtsan, feature = "cabi", feature = "test-host"))]

use hapto_host::cabi::TestHost;
use hapto_host::layout::{Stereo, Width};
use hapto_host::{
    AuCodes, Block, Category, Parts, Plugin, PluginInfo, ProcessSetup, Processor, Status,
};
use hapto_params::{Declarer, Float, FloatDecl, Params};

/// Allocates in every block, as no processor may.
struct Allocates;

impl Processor for Allocates {
    type Layout = Stereo;

    fn process(&mut self, mut block: Block<'_, Stereo>) -> Status {
        let scratch = std::hint::black_box(vec![0.0_f32; block.audio.frames()]);
        for channel in block.audio.channels_mut() {
            channel.copy_from_slice(&scratch);
        }
        Status::Continue
    }
}

struct Gain {
    _gain: Float,
}

impl Params for Gain {
    fn declare(p: &mut Declarer) -> Self {
        Gain {
            _gain: p.add(FloatDecl::new("gain", "Gain", 0.0..=1.0, 0.0)),
        }
    }
}

struct Canary;

impl Plugin for Canary {
    const INFO: PluginInfo = PluginInfo {
        name: "Canary",
        vendor: "Hapto",
        url: "",
        email: "",
        description: "",
        version: "0.1.0",
        id: "dev.hapto.rtsan-canary",
        vst3_class_id: [9; 16],
        au_codes: AuCodes {
            subtype: *b"cnry",
            manufacturer: *b"Hapt",
        },
        category: Category::Effect,
        features: &[],
        layouts: &[],
        midi_input: false,
        midi_output: false,
    };
    type Params = Gain;
    type Processor = Allocates;

    fn new(_: Gain, _: Parts) -> Self {
        Canary
    }

    fn activate(&mut self, _setup: &ProcessSetup) -> Allocates {
        Allocates
    }
}

#[test]
fn an_allocation_inside_the_marked_process_aborts() {
    let host = TestHost::<Canary>::new();
    assert_eq!(host.activate(48_000.0, 64, Width::STEREO), Ok(()));
    let (mut left, mut right) = ([1.0_f32; 64], [1.0_f32; 64]);
    host.process_in_place(&mut [&mut left, &mut right], 64);
    // Reached only when the sanitizer let the allocation through.
    assert_eq!(left, [0.0; 64]);
}
