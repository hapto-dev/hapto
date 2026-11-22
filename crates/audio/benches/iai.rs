//! Instruction counts for walking audio: the same gain over a stereo block,
//! walked frame by frame, channel by channel, and as one call, and a mono
//! source added into every channel frame by frame. Run by CI with
//! iai-callgrind, which compares them with the pull request's base; they say
//! whether walking frames costs more than walking channels.

#![allow(missing_docs, reason = "the benchmark macros generate public items")]

use std::hint::black_box;

use hapto_audio::AudioBuf;
use hapto_audio::layout::{Stereo, Width};
use iai_callgrind::{library_benchmark, library_benchmark_group, main};

/// A block's frames: a common host block size.
const FRAMES: usize = 512;

/// A stereo block of a little signal.
fn stereo() -> AudioBuf<Stereo> {
    let mut buffer = AudioBuf::<Stereo>::new(Width::STEREO, FRAMES);
    buffer
        .audio()
        .add_to_all((0..FRAMES).map(|frame| frame as f32 * 1e-3));
    buffer
}

#[library_benchmark]
#[bench::block(stereo())]
fn gain_by_frame(mut buffer: AudioBuf<Stereo>) -> AudioBuf<Stereo> {
    let gain = black_box(0.5_f32);
    for [left, right] in buffer.audio().frames_mut() {
        *left *= gain;
        *right *= gain;
    }
    buffer
}

#[library_benchmark]
#[bench::block(stereo())]
fn gain_by_channel(mut buffer: AudioBuf<Stereo>) -> AudioBuf<Stereo> {
    let gain = black_box(0.5_f32);
    for channel in buffer.audio().channels_mut() {
        for sample in channel {
            *sample *= gain;
        }
    }
    buffer
}

#[library_benchmark]
#[bench::block(stereo())]
fn gain_in_one_call(mut buffer: AudioBuf<Stereo>) -> AudioBuf<Stereo> {
    buffer.audio().apply_gain(black_box(0.5));
    buffer
}

#[library_benchmark]
#[bench::block(AudioBuf::stereo(FRAMES))]
fn a_mono_source_into_every_channel(mut buffer: AudioBuf) -> AudioBuf {
    let level = black_box(0.25_f32);
    for frame in buffer.audio().frames_mut() {
        frame.add(level);
    }
    buffer
}

library_benchmark_group!(
    name = audio;
    benchmarks = gain_by_frame, gain_by_channel, gain_in_one_call, a_mono_source_into_every_channel
);
main!(library_benchmark_groups = audio);
