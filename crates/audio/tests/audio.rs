//! Audio as a processor walks it: frames and channels in order and in
//! range, the layout's channel count kept, and the mistakes `Audio::new` and
//! `AudioBuf::new` refuse.

use hapto_audio::layout::{AnyLayout, MAX_CHANNELS, Mono, Stereo, StereoOut, Width};
use hapto_audio::{Audio, AudioBuf};

/// Stereo audio whose left channel counts 0, 1, 2… and right counts 10,
/// 11, 12…, `frames` long.
fn counting(frames: usize) -> AudioBuf<Stereo> {
    let mut buffer = AudioBuf::<Stereo>::new(Width::STEREO, frames);
    let mut audio = buffer.audio();
    for (start, channel) in [0.0, 10.0].into_iter().zip(audio.channels_mut()) {
        for (index, sample) in channel.iter_mut().enumerate() {
            *sample = start + index as f32;
        }
    }
    buffer
}

#[test]
fn stereo_frames_come_in_time_order_left_then_right() {
    let mut buffer = counting(3);
    let frames: Vec<(f32, f32)> = buffer
        .audio()
        .frames_mut()
        .map(|[left, right]| (*left, *right))
        .collect();
    assert_eq!(frames, [(0.0, 10.0), (1.0, 11.0), (2.0, 12.0)]);
}

#[test]
fn a_frame_writes_back_into_its_own_moment() {
    let mut buffer = counting(3);
    for (index, [left, right]) in buffer.audio().frames_mut().enumerate() {
        if index == 1 {
            (*left, *right) = (-1.0, -2.0);
        }
    }
    let channels: Vec<&[f32]> = buffer.channels().collect();
    assert_eq!(channels, [[0.0, -1.0, 2.0], [10.0, -2.0, 12.0]]);
}

#[test]
fn frames_count_exactly_and_end() {
    let mut buffer = counting(5);
    let mut audio = buffer.audio();
    let mut frames = audio.frames_mut();
    assert_eq!(frames.len(), 5);
    frames.nth(3);
    assert_eq!(frames.len(), 1);
    assert!(frames.next().is_some());
    assert!(frames.next().is_none());
}

#[test]
fn audio_of_no_channels_still_counts_its_frames() {
    let mut channels: [&mut [f32]; 0] = [];
    let mut audio: Audio<'_> = Audio::new(&mut channels, 4);
    assert_eq!(audio.frames(), 4);
    assert_eq!(audio.channels().len(), 0);
    assert!(audio.frames_mut().all(|frame| frame.is_empty()));
    assert_eq!(audio.frames_mut().count(), 4);
    assert_eq!(audio.peak(), 0.0);
}

#[test]
fn a_mono_frame_is_its_one_sample() {
    let mut buffer = AudioBuf::<Mono>::new(Width::MONO, 3);
    for (index, sample) in buffer.audio().frames_mut().enumerate() {
        *sample = index as f32;
    }
    assert_eq!(buffer.channels().next(), Some(&[0.0, 1.0, 2.0][..]));
}

#[test]
fn a_frame_of_any_layout_holds_every_channel() {
    let mut buffer = AudioBuf::<AnyLayout>::new(Width::new(6, 6), 2);
    let mut audio = buffer.audio();
    for mut frame in audio.frames_mut() {
        assert_eq!(frame.len(), 6);
        for (sample, value) in frame.samples_mut().zip(1..) {
            *sample = value as f32;
        }
    }
    let firsts: Vec<f32> = audio.channels().map(|channel| channel[0]).collect();
    assert_eq!(firsts, [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
}

#[test]
fn new_lends_only_the_frames_asked_for() {
    let (mut left, mut right) = ([1.0_f32; 6], [1.0_f32; 6]);
    let mut channels = [&mut left[..], &mut right[..]];
    let mut audio: Audio<'_, Stereo> = Audio::new(&mut channels, 2);
    assert_eq!(audio.frames(), 2);
    assert!(audio.channels().all(|channel| channel.len() == 2));
    audio.clear();
    assert_eq!(left, [0.0, 0.0, 1.0, 1.0, 1.0, 1.0]);
    assert_eq!(right, left);
}

#[test]
fn add_to_all_stops_at_the_shorter() {
    let mut buffer = AudioBuf::stereo(4);
    buffer.audio().add_to_all([1.0, 2.0]);
    buffer.audio().add_to_all([1.0; 10]);
    assert!(
        buffer
            .channels()
            .all(|channel| channel == [2.0, 3.0, 1.0, 1.0])
    );
}

#[test]
fn peak_is_the_largest_magnitude() {
    let mut buffer = AudioBuf::stereo(3);
    let mut audio = buffer.audio();
    audio
        .channel_mut(1)
        .unwrap()
        .copy_from_slice(&[0.25, -0.75, 0.5]);
    assert_eq!(audio.peak(), 0.75);
    audio.apply_gain(2.0);
    assert_eq!(audio.peak(), 1.5);
}

#[test]
fn peak_is_nan_when_a_sample_is() {
    let mut buffer = AudioBuf::stereo(4);
    let mut audio = buffer.audio();
    audio
        .channel_mut(0)
        .unwrap()
        .copy_from_slice(&[0.3, f32::NAN, 0.1, 0.9]);
    assert!(
        audio.peak().is_nan(),
        "a NaN anywhere, before larger samples too"
    );
}

#[test]
fn a_typed_layout_lends_its_audio_to_code_for_any_layout() {
    let mut buffer = counting(2);
    let mut audio = buffer.audio();
    let mut any = audio.as_any_layout();
    assert_eq!((any.channels().len(), any.frames()), (2, 2));
    for frame in any.frames_mut() {
        frame.add(100.0);
    }
    let channels: Vec<&[f32]> = buffer.channels().collect();
    assert_eq!(channels, [[100.0, 101.0], [110.0, 111.0]]);
}

#[test]
fn a_channel_past_the_last_is_none() {
    let mut buffer = AudioBuf::stereo(1);
    let mut audio = buffer.audio();
    assert!(audio.channel_mut(1).is_some());
    assert!(audio.channel_mut(2).is_none());
}

#[test]
fn channels_walk_back_to_front_too() {
    let mut buffer = counting(1);
    let lasts: Vec<f32> = buffer.audio().channels_mut().rev().map(|c| c[0]).collect();
    assert_eq!(lasts, [10.0, 0.0]);
}

#[test]
fn an_instrument_runs_at_no_input_and_two_outputs() {
    let buffer = AudioBuf::<StereoOut>::new(Width::new(0, 2), 8);
    assert_eq!(buffer.channels().len(), 2);
}

#[test]
#[should_panic(expected = "at most 16 channels")]
fn new_refuses_more_than_the_most_channels() {
    let mut samples = vec![vec![0.0_f32; 1]; MAX_CHANNELS + 1];
    let mut channels: Vec<&mut [f32]> = samples.iter_mut().map(Vec::as_mut_slice).collect();
    let _: Audio<'_> = Audio::new(&mut channels, 1);
}

#[test]
#[should_panic(expected = "fixes its channels")]
fn new_refuses_a_count_the_layout_does_not_fix() {
    let mut left = [0.0_f32; 4];
    let mut channels = [&mut left[..]];
    let _: Audio<'_, Stereo> = Audio::new(&mut channels, 4);
}

#[test]
#[should_panic(expected = "fewer than the 8 frames")]
fn new_refuses_a_channel_shorter_than_the_frames() {
    let (mut left, mut right) = ([0.0_f32; 8], [0.0_f32; 4]);
    let mut channels = [&mut left[..], &mut right[..]];
    let _: Audio<'_> = Audio::new(&mut channels, 8);
}

#[test]
#[should_panic(expected = "this layout runs at")]
fn a_buffer_refuses_a_width_its_layout_does_not_run_at() {
    let _ = AudioBuf::<StereoOut>::new(Width::STEREO, 8);
}

#[test]
#[should_panic(expected = "at most 16 channels")]
fn a_buffer_refuses_more_than_the_most_channels() {
    let _ = AudioBuf::<AnyLayout>::new(Width::new(0, MAX_CHANNELS + 1), 8);
}
