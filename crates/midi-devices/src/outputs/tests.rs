//! The app's MIDI out with no device: what the writer hands each output
//! that is read, when each packet is due, and what is dropped or lost.

use std::time::Duration;

use hapto_ump::Event;

use super::*;

const START: Instant = Instant::from_nanos(1_000_000_000);

fn later(duration: Duration) -> Instant {
    START.after(duration)
}

fn first_words(taken: &[(Instant, [u32; 4])]) -> Vec<(Instant, u32)> {
    taken.iter().map(|&(at, words)| (at, words[0])).collect()
}

#[test]
fn at_frames_a_block_goes_to_each_open_output_as_midi_1_due_at_its_frame() {
    let (mut outputs, mut writer) = outputs(16);
    outputs.set_timing(Timing::AtFrames);
    // Output 0 is read, as an open device reads it; output 1 has none.
    assert_eq!(outputs.take(0), Some(Vec::new()));
    let sent = [
        Event::new(0, [0x4090_3C00, 0xFFFF_0000, 0, 0]),
        Event::new(480, [0x40B0_0700, u32::MAX, 0, 0]),
    ];
    writer.write(START, 48_000.0, &sent);
    let taken = outputs.take(0).expect("no device there");
    assert_eq!(
        first_words(&taken),
        [
            (START, 0x2090_3C7F),
            (later(Duration::from_millis(10)), 0x20B0_077F)
        ],
        "480 frames at 48 kHz are 10 ms"
    );
    assert_eq!(outputs.take(1), Some(Vec::new()), "nothing for no device");
    assert_eq!((writer.dropped(), writer.lost()), (0, 0));
}

#[test]
fn by_default_a_blocks_midi_is_due_at_once_as_juce_sends_it() {
    let (mut outputs, mut writer) = outputs(16);
    assert_eq!(outputs.timing(), Timing::Immediate);
    outputs.take(0);
    let sent = [
        Event::new(0, [0x4090_3C00, 0xFFFF_0000, 0, 0]),
        Event::new(480, [0x4080_3C00, 0, 0, 0]),
    ];
    writer.write(START, 48_000.0, &sent);
    let taken = outputs.take(0).expect("no device there");
    // Both due before any moment the clock can tell, so sent as they come,
    // in the order the plugin sent them.
    let at_once = Instant::from_nanos(0);
    assert_eq!(
        first_words(&taken),
        [(at_once, 0x2090_3C7F), (at_once, 0x2080_3C00)]
    );
}

#[test]
fn what_midi_1_has_no_message_for_is_lost_and_a_block_with_no_output_costs_nothing() {
    let (mut outputs, mut writer) = outputs(16);
    let unsent = [
        // A note's own pitch bend, which MIDI 1.0 has no message for.
        Event::new(0, [0x4060_3C00, 0x8000_0000, 0, 0]),
        // A utility message, which has no MIDI 1.0 bytes.
        Event::new(0, [0x0000_0000, 0, 0, 0]),
    ];
    // No output is read: the block is not even downgraded.
    writer.write(START, 48_000.0, &unsent);
    assert_eq!(writer.lost(), 0);
    outputs.take(0);
    writer.write(START, 48_000.0, &unsent);
    assert_eq!(outputs.take(0), Some(Vec::new()));
    assert_eq!(writer.lost(), 2);
}

#[test]
fn a_registered_controller_goes_out_as_its_four_and_a_sysex_as_its_packets() {
    let (mut outputs, mut writer) = outputs(16);
    outputs.set_timing(Timing::AtFrames);
    outputs.take(0);
    // The bend range, 12 semitones, then a SysEx of eight bytes.
    let sent = [
        Event::new(
            0,
            [0x4020_0000, hapto_ump::scale::up_14_to_32(12 << 7), 0, 0],
        ),
        Event::new(1, [0x3016_0102, 0x0304_0506, 0, 0]),
        Event::new(1, [0x3032_0708, 0, 0, 0]),
    ];
    writer.write(START, 1_000.0, &sent);
    let taken = outputs.take(0).expect("no device there");
    let at_one = later(Duration::from_millis(1));
    assert_eq!(
        first_words(&taken),
        [
            (START, 0x20B0_6500),
            (START, 0x20B0_6400),
            (START, 0x20B0_060C),
            (START, 0x20B0_2600),
            (at_one, 0x3016_0102),
            (at_one, 0x3032_0708),
        ]
    );
}

#[test]
fn past_an_outputs_room_a_packet_is_dropped_and_counted() {
    let (mut outputs, mut writer) = outputs(4);
    outputs.take(0);
    let notes: Vec<Event> = (0..6)
        .map(|key| Event::new(0, [0x4090_0000 | key << 8, 0xFFFF_0000, 0, 0]))
        .collect();
    writer.write(START, 48_000.0, &notes);
    assert_eq!(outputs.take(0).map(|taken| taken.len()), Some(4));
    assert_eq!(writer.dropped(), 2);
}

#[test]
fn an_input_is_not_an_output() {
    let (mut outputs, _writer) = outputs(4);
    let keyboard = Device::for_tests("Keys", Direction::Input);
    assert_eq!(outputs.open(&keyboard), Err(OpenError::NotAnOutput));
    assert_eq!(outputs.devices().count(), 0);
}
