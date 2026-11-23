//! A device's buffers play through the plugin in blocks cut as the plan
//! says, the sound the same whatever the cut, with the test signal on every
//! input and the plugin's outputs on the device's channels.

use std::sync::{Arc, Mutex};

use hapto_host::Event;
use hapto_host::layout::Width;

use super::{Plan, Player, RECENT, Render, preview};
use crate::settings::Blocks;
use crate::signal::{Generator, Signal};
use hapto_midi_devices::{Instant, inputs, outputs};

/// Writes each input, doubled, to the output of the same number, and keeps
/// the length of each block it was given.
#[derive(Debug, Default, Clone)]
struct Doubler {
    lengths: Arc<Mutex<Vec<usize>>>,
}

impl Render for Doubler {
    fn render(
        &mut self,
        inputs: &[&[f32]],
        outputs: &mut [&mut [f32]],
        _midi: &[Event],
        _sent: &mut dyn FnMut(&[Event]),
    ) {
        let length = outputs.first().map_or(0, |c| c.len());
        self.lengths.lock().expect("not poisoned").push(length);
        for (index, output) in outputs.iter_mut().enumerate() {
            for (frame, sample) in output.iter_mut().enumerate() {
                *sample = inputs.get(index).map_or(0.25, |input| input[frame] * 2.0);
            }
        }
    }
}

fn player(blocks: Blocks, max: usize) -> (Player<Doubler>, Doubler, Arc<Plan>) {
    let doubler = Doubler::default();
    let plan = Plan::new(blocks, Signal::Sine);
    let player = Player::new(
        doubler.clone(),
        Width::STEREO,
        2,
        48_000.0,
        max,
        Arc::clone(&plan),
    );
    (player, doubler, plan)
}

/// Plays `buffers` device buffers of `frames` frames each, and returns the
/// left channel the device was given.
fn left(player: &mut Player<Doubler>, buffers: usize, frames: usize) -> Vec<f32> {
    let mut played = Vec::new();
    for _ in 0..buffers {
        let mut data = vec![9.0_f32; 2 * frames];
        player.play(&mut data);
        played.extend(data.iter().step_by(2));
    }
    played
}

fn doubled_sine(frames: usize) -> Vec<f32> {
    let mut signal = vec![0.0; frames];
    Generator::new(Signal::Sine, 48_000.0).fill(&mut signal);
    signal.iter().map(|s| s * 2.0).collect()
}

#[test]
fn the_sound_is_the_same_whatever_the_cut() {
    for blocks in Blocks::ALL {
        for buffer in [480, 64, 2_500] {
            let (mut player, _, _) = player(blocks, 256);
            let played = left(&mut player, 7, buffer);
            assert_eq!(played, doubled_sine(7 * buffer), "{blocks:?}, {buffer}");
        }
    }
}

#[test]
fn as_the_device_asks_cuts_each_buffer_at_the_maximum() {
    let (mut player, doubler, plan) = player(Blocks::Device, 256);
    left(&mut player, 2, 600);
    let lengths = doubler.lengths.lock().expect("not poisoned").clone();
    assert_eq!(lengths, [256, 256, 88, 256, 256, 88]);
    assert_eq!(plan.played(), 6);
    assert_eq!(plan.recent(), [256, 256, 88, 256, 256, 88]);
}

#[test]
fn as_the_device_asks_cuts_at_a_maximum_past_the_part_size() {
    // At 192 kHz a 10 ms buffer is 1,920 frames, under a maximum of 2,048.
    let (mut short, doubler, _) = player(Blocks::Device, 2048);
    left(&mut short, 1, 1920);
    assert_eq!(*doubler.lengths.lock().expect("ok"), [1920], "one block");
    let (mut long, doubler, _) = player(Blocks::Device, 2048);
    left(&mut long, 1, 5000);
    assert_eq!(*doubler.lengths.lock().expect("ok"), [2048, 2048, 904]);
}

#[test]
fn always_the_maximum_and_random_stay_within_it() {
    let (mut fixed, doubler, _) = player(Blocks::Fixed, 128);
    left(&mut fixed, 5, 300);
    assert!(
        doubler
            .lengths
            .lock()
            .expect("ok")
            .iter()
            .all(|&l| l == 128)
    );

    let (mut random, doubler, _) = player(Blocks::Random, 128);
    left(&mut random, 20, 300);
    let lengths = doubler.lengths.lock().expect("ok").clone();
    assert!(
        lengths.iter().all(|&l| (1..=128).contains(&l)),
        "{lengths:?}"
    );
    assert!(lengths.iter().any(|&l| l != lengths[0]), "they vary");
}

#[test]
fn awkward_blocks_hold_one_frame_none_and_one_past_the_maximum() {
    let (mut player, doubler, _) = player(Blocks::Awkward, 64);
    left(&mut player, 10, 100);
    let lengths = doubler.lengths.lock().expect("ok").clone();
    for length in [1, 0, 64, 65, 63] {
        assert!(lengths.contains(&length), "{length} in {lengths:?}");
    }
}

#[test]
fn awkward_blocks_stay_within_a_maximum_of_one_frame() {
    let (mut player, doubler, _) = player(Blocks::Awkward, 1);
    left(&mut player, 10, 4);
    let lengths = doubler.lengths.lock().expect("ok").clone();
    assert!(lengths.iter().all(|&l| l <= 2), "{lengths:?}");
}

#[test]
fn a_plan_changed_while_blocks_play_cuts_the_next_ones() {
    let (mut player, doubler, plan) = player(Blocks::Fixed, 64);
    left(&mut player, 1, 64);
    plan.set(Blocks::Awkward);
    assert_eq!(plan.blocks(), Blocks::Awkward);
    left(&mut player, 1, 64);
    assert!(doubler.lengths.lock().expect("ok").contains(&0));
}

#[test]
fn a_signal_changed_while_blocks_play_reaches_the_next_block() {
    let (mut player, _, plan) = player(Blocks::Fixed, 64);
    assert_ne!(left(&mut player, 1, 64), [0.0; 64], "the sine");
    plan.set_signal(Signal::Silence);
    assert_eq!(plan.signal(), Signal::Silence);
    assert_eq!(left(&mut player, 1, 64), [0.0; 64]);
}

#[test]
fn the_plan_keeps_the_latest_lengths_the_last_one_last() {
    let (mut player, _, plan) = player(Blocks::Fixed, 8);
    assert!(plan.recent().is_empty(), "none played");
    left(&mut player, 50, 8);
    assert_eq!(plan.played(), 50);
    assert_eq!(plan.recent(), [8; RECENT]);
    plan.set(Blocks::Awkward);
    left(&mut player, 1, 8);
    let played = usize::try_from(plan.played()).expect("a few");
    let awkward = played - 50;
    let cycle = [1, 0, 8, 3, 9, 7, 8, 0, 5];
    let recent = plan.recent();
    assert_eq!(recent.len(), RECENT);
    assert_eq!(recent[RECENT - awkward..], cycle[..awkward], "{recent:?}");
}

#[test]
fn a_mono_plugin_plays_on_every_channel_and_a_missing_output_is_silent() {
    let mono = |outputs, channels| {
        Player::new(
            Doubler::default(),
            Width::new(0, outputs),
            channels,
            48_000.0,
            64,
            Plan::new(Blocks::Device, Signal::Silence),
        )
    };
    let mut data = vec![9.0_f32; 3 * 4];
    mono(1, 3).play(&mut data);
    assert_eq!(data, [0.25; 12], "the one output everywhere");
    let mut data = vec![9.0_f32; 3 * 2];
    mono(2, 3).play(&mut data);
    assert_eq!(data, [0.25, 0.25, 0.0, 0.25, 0.25, 0.0]);
}

#[test]
fn the_preview_is_what_a_player_cuts() {
    for blocks in Blocks::ALL {
        for max in [1, 64] {
            let (mut player, doubler, _plan) = player(blocks, max);
            left(&mut player, 4, 448);
            let played = doubler.lengths.lock().expect("not poisoned").clone();
            assert_eq!(
                played[..12],
                preview(blocks, max, 448, 12)[..],
                "{blocks:?} at most {max}"
            );
        }
    }
}

/// A block's length, and each of its packets' frame and first word.
type Heard = (usize, Vec<(u32, u32)>);

/// Keeps what MIDI each block carried.
#[derive(Debug, Default, Clone)]
struct Listener {
    blocks: Arc<Mutex<Vec<Heard>>>,
}

impl Render for Listener {
    fn render(
        &mut self,
        _inputs: &[&[f32]],
        outputs: &mut [&mut [f32]],
        midi: &[Event],
        _sent: &mut dyn FnMut(&[Event]),
    ) {
        let length = outputs.first().map_or(0, |c| c.len());
        let packets = midi
            .iter()
            .map(|event| (event.offset, event.words[0]))
            .collect();
        self.blocks
            .lock()
            .expect("not poisoned")
            .push((length, packets));
    }
}

#[test]
fn the_apps_midi_inputs_reach_the_plugin_in_its_blocks() {
    let listener = Listener::default();
    let plan = Plan::new(Blocks::Fixed, Signal::Silence);
    let (mut inputs, reader) = inputs(16);
    let mut player =
        Player::new(listener.clone(), Width::STEREO, 2, 48_000.0, 64, plan).with_midi(reader);
    // Long before any block: the first block's first frame, upgraded.
    assert!(inputs.push(0, Instant::from_nanos(0), [0x2090_3C7F, 0, 0, 0]));
    assert!(inputs.push(4, Instant::from_nanos(1), [0x2090_3E7F, 0, 0, 0]));
    let mut buffer = vec![0.0_f32; 128 * 2];
    player.play(&mut buffer);
    let blocks = listener.blocks.lock().expect("not poisoned").clone();
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0], (64, vec![(0, 0x4090_3C00), (0, 0x4090_3E00)]));
    assert_eq!(blocks[1], (64, Vec::new()), "each packet once");
}

/// Sends a note-on at the first frame of every block it plays.
struct Sender;

impl Render for Sender {
    fn render(
        &mut self,
        _inputs: &[&[f32]],
        _outputs: &mut [&mut [f32]],
        _midi: &[Event],
        sent: &mut dyn FnMut(&[Event]),
    ) {
        sent(&[Event::new(0, [0x4090_3C00, 0xFFFF_0000, 0, 0])]);
    }
}

#[test]
fn each_blocks_midi_is_due_where_its_frames_fall_in_the_devices_buffer() {
    let plan = Plan::new(Blocks::Fixed, Signal::Silence);
    let (mut outputs, writer) = outputs(16);
    outputs.take(0);
    let mut player =
        Player::new(Sender, Width::STEREO, 2, 48_000.0, 64, plan).with_midi_out(writer);
    // One buffer of 256 frames: four blocks of 64, rendered back to back.
    let before = Instant::now();
    let mut buffer = vec![0.0_f32; 256 * 2];
    player.play(&mut buffer);
    let taken = outputs.take(0).expect("no device there");
    let dues: Vec<u64> = taken.iter().map(|(at, _)| at.nanos()).collect();
    assert_eq!(dues.len(), 4);
    assert!(dues[0] >= before.nanos(), "the first when the device asked");
    // 64 frames at 48 kHz apart, as their audio is, not as fast as they
    // were rendered.
    let gaps: Vec<u64> = dues.windows(2).map(|pair| pair[1] - pair[0]).collect();
    assert!(
        gaps.iter().all(|gap| gap.abs_diff(1_333_333) <= 1),
        "{gaps:?}"
    );
    // Downgraded, as a device takes it.
    assert_eq!(taken[0].1[0], 0x2090_3C7F);
}
