//! Instruction counts for the example EQ's audio path: one block of noise
//! through the processor, steady and after a parameter moved. Run by CI with
//! iai-callgrind, which compares them with the pull request's base.

#![allow(missing_docs, reason = "the benchmark macros generate public items")]

use std::hint::black_box;

use hapto_example_eq::ExampleEq;
use hapto_host::layout::Width;
use hapto_host::{Audio, Block, EventList, Midi, MidiOut, Parts, Plugin, ProcessSetup, Processor};
use hapto_params::{AudioParams, ParamId};
use iai_callgrind::{library_benchmark, library_benchmark_group, main};

const FRAMES: usize = 256;

struct Rig {
    processor: <ExampleEq as Plugin>::Processor,
    audio: AudioParams,
    gain: ParamId,
    out_events: EventList,
    left: Vec<f32>,
    right: Vec<f32>,
}

/// An active EQ with a block already processed, so every filter is tuned.
fn rig() -> Rig {
    let (params, parts) = Parts::for_plugin::<ExampleEq>().expect("valid declarations");
    let set = std::sync::Arc::clone(parts.set());
    let mut plugin = ExampleEq::new(params, parts);
    let processor = plugin.activate(&ProcessSetup::new(48_000.0, FRAMES, Width::STEREO));
    let mut state: u32 = 0x1234_5678;
    let mut noise = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state as i32 as f32 / 2_147_483_648.0
    };
    let left: Vec<f32> = (0..FRAMES).map(|_| noise()).collect();
    let right = left.iter().map(|sample| -sample).collect();
    let mut rig = Rig {
        processor,
        audio: set.audio(),
        gain: set.id("mid.gain").expect("declared"),
        out_events: EventList::with_capacity(0),
        left,
        right,
    };
    run(&mut rig);
    rig
}

fn run(rig: &mut Rig) {
    let mut channels: [&mut [f32]; 2] = [&mut rig.left, &mut rig.right];
    rig.processor.process(Block::new(
        Audio::new(&mut channels, FRAMES),
        Midi::default(),
        MidiOut::new(&mut rig.out_events),
    ));
}

#[library_benchmark]
#[bench::steady(rig())]
fn a_block(mut rig: Rig) -> Rig {
    run(black_box(&mut rig));
    rig
}

#[library_benchmark]
#[bench::one_band_moved(rig())]
fn a_block_after_a_parameter_moved(mut rig: Rig) -> Rig {
    rig.audio.host_write(rig.gain, black_box(0.8));
    run(black_box(&mut rig));
    rig
}

library_benchmark_group!(
    name = eq;
    benchmarks = a_block, a_block_after_a_parameter_moved
);
main!(library_benchmark_groups = eq);
