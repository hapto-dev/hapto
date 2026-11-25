//! Instruction counts for a mono source written into a stereo block, through
//! a typed layout's own frames and placed by pan through a `VoiceAudio`'s.
//!
//! CI runs them with iai-callgrind and compares them with the pull request's
//! base, which shows whether placing a voice keeps it at a typed layout's
//! speed.

#![allow(missing_docs, reason = "the benchmark macros generate public items")]

use std::hint::black_box;

use hapto_audio::AudioBuf;
use hapto_audio::layout::StereoOut;
use hapto_midi::{Centred, VoiceAudio};
use iai_callgrind::{library_benchmark, library_benchmark_group, main};

/// A block's frames: a common host block size.
const FRAMES: usize = 512;

#[library_benchmark]
#[bench::block(AudioBuf::<StereoOut>::new(FRAMES))]
fn a_mono_source_into_a_typed_frame(mut buffer: AudioBuf<StereoOut>) -> AudioBuf<StereoOut> {
    let level = black_box(0.25_f32);
    for [left, right] in buffer.audio().frames_mut() {
        *left += level;
        *right += level;
    }
    buffer
}

#[library_benchmark]
#[bench::block(AudioBuf::<StereoOut>::new(FRAMES))]
fn a_mono_voice_placed_by_pan(mut buffer: AudioBuf<StereoOut>) -> AudioBuf<StereoOut> {
    let level = black_box(0.25_f32);
    let mut out = VoiceAudio::new(buffer.audio(), black_box(Centred::new(0.3)));
    for frame in out.frames_mut() {
        frame.add(level);
    }
    buffer
}

library_benchmark_group!(
    name = voices;
    benchmarks = a_mono_source_into_a_typed_frame, a_mono_voice_placed_by_pan
);
main!(library_benchmark_groups = voices);
