//! The probe through the C interface, as the JUCE adapter drives it: a note
//! sounds, every message comes back out as it came, and a released note
//! falls silent and lets the block sleep.

use hapto_example_midi_probe::MidiProbe;
use hapto_host::cabi::{HaptoPacket, MidiBlock, STATUS_SLEEP, TestHost};
use hapto_host::layout::Width;
use hapto_host::midi::controller::VOLUME;
use hapto_host::midi::{Channel, Group, Key, Message, NoteOn, packets_of};

const FRAMES: usize = 256;

/// The packets `messages` go out as, as a host hands them in.
fn packets<'m>(messages: impl IntoIterator<Item = (usize, Message<'m>)>) -> Vec<HaptoPacket> {
    packets_of(messages)
        .into_iter()
        .map(HaptoPacket::from)
        .collect()
}

/// One block with `midi_in`, the host taking MIDI 2.0 out: its status, its
/// loudest sample, and what came back out.
fn play(host: &TestHost<MidiProbe>, midi_in: &[HaptoPacket]) -> (u32, f32, Vec<HaptoPacket>) {
    let (mut left, mut right) = ([0.0_f32; FRAMES], [0.0_f32; FRAMES]);
    let mut out = [HaptoPacket::NONE; 16];
    let midi = MidiBlock {
        midi_in,
        midi_out: &mut out,
        midi_1_out: false,
    };
    let (status, sent) = host.process_with_midi(&mut [&mut left[..], &mut right[..]], FRAMES, midi);
    let peak = left
        .iter()
        .chain(&right)
        .fold(0.0_f32, |peak, s| peak.max(s.abs()));
    (status, peak, out[..sent].to_vec())
}

fn running() -> TestHost<MidiProbe> {
    let host = TestHost::<MidiProbe>::new();
    assert_eq!(host.activate(48_000.0, FRAMES, Width::STEREO_OUT), Ok(()));
    host
}

#[test]
fn a_note_sounds_and_every_message_comes_back_out_as_it_came() {
    let host = running();
    // A4 at full velocity, then the channel's volume, at their frames.
    let sent = packets([
        (0, NoteOn::new(Key::A4, 1.0).into()),
        (64, VOLUME.message(Group::FIRST, Channel::FIRST, 1.0)),
    ]);
    let (_, peak, out) = play(&host, &sent);
    assert!(peak > 0.05, "the note sounds: {peak}");
    let words = |packets: &[HaptoPacket]| {
        packets
            .iter()
            .map(|p| (p.frame, p.words))
            .collect::<Vec<_>>()
    };
    assert_eq!(words(&out), words(&sent));
    assert_eq!(host.faults(), 0);
}

#[test]
fn a_released_note_falls_silent_and_the_block_sleeps() {
    let host = running();
    let note = NoteOn::new(Key::A4, 1.0);
    play(&host, &packets([(0, note.into())]));
    let (_, _, out) = play(&host, &packets([(0, note.off(0.5).into())]));
    assert_eq!(out.len(), 1, "the note-off comes back out");
    // Its release falls 60 dB in a fifth of a second: under 40 blocks.
    let mut last = (0, f32::MAX);
    for _ in 0..60 {
        let (status, peak, _) = play(&host, &[]);
        last = (status, peak);
        if status == STATUS_SLEEP {
            break;
        }
    }
    assert_eq!(last.0, STATUS_SLEEP, "the probe sleeps once its notes end");
    assert!(last.1 < 1e-3, "and is silent: {}", last.1);
}
