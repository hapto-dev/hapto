//! A block cut into spans that start at each message's frame, are no longer
//! than a limit, and cover the block once with every message where it
//! belongs.

use hapto_host::layout::Mono;
use hapto_host::midi::{Message, MidiList};
use hapto_host::{AudioBuf, Event, EventList, Midi, Spans};
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

/// Returns a list with channel pressure at each of `frames`, whose value is
/// the message's place, so a test can follow each.
fn list(frames: &[u32], block: usize) -> MidiList {
    let mut packets = EventList::with_capacity(frames.len());
    for (place, &frame) in frames.iter().enumerate() {
        packets.push(Event::new(frame, [0x40D0_0000, place as u32, 0, 0]));
    }
    let mut list = MidiList::new();
    list.read(&packets, block);
    list
}

/// Returns each span's start and length, and the places of the messages
/// that it got.
fn spans(midi: Midi<'_>, block: usize, most: usize) -> Vec<(usize, usize, Vec<u32>)> {
    let mut buffer = AudioBuf::<Mono>::new(block);
    Spans::new(buffer.audio(), midi)
        .at_most(most)
        .map(|span| {
            let places = span
                .midi
                .iter()
                .map(|(frame, message)| {
                    assert_eq!(frame, 0, "every message at the span's first frame");
                    match message {
                        Message::Pressure { value, .. } => (value * u32::MAX as f32).round() as u32,
                        other => panic!("{other:?}"),
                    }
                })
                .collect();
            (span.start, span.audio.frames(), places)
        })
        .collect()
}

#[test]
fn a_block_splits_at_each_frame_a_message_arrives_at() {
    let list = list(&[0, 3, 3, 7], 10);
    let got = spans(list.midi(), 10, usize::MAX);
    assert_eq!(
        got,
        [(0, 3, vec![0]), (3, 4, vec![1, 2]), (7, 3, vec![3])],
        "messages at one frame come together"
    );
}

#[test]
fn a_block_with_no_midi_is_one_span_and_a_block_of_no_frames_none() {
    assert_eq!(spans(Midi::default(), 16, usize::MAX), [(0, 16, vec![])]);
    assert_eq!(spans(Midi::default(), 0, usize::MAX), []);
}

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "tests/spans.proptest-regressions",
        ))),
        cases: 1024,
        ..ProptestConfig::default()
    })]

    /// However the messages fall and however short the spans must be:
    ///
    /// - They cover the block once, in order.
    /// - None is empty or longer than the most.
    /// - One starts at every message's frame.
    /// - Every message comes once, with the span that starts at its frame.
    #[test]
    fn spans_cover_the_block_once_with_every_message_where_it_arrived(
        block in 1_usize..64,
        most in 1_usize..80,
        mut frames in prop::collection::vec(0_u32..70, 0..12),
    ) {
        frames.sort_unstable();
        let list = list(&frames, block);
        let got = spans(list.midi(), block, most);
        let mut at = 0;
        for (start, length, _) in &got {
            prop_assert_eq!(*start, at);
            prop_assert!(*length >= 1 && *length <= most);
            at += length;
        }
        prop_assert_eq!(at, block);
        // A message past the block arrives at the block's last frame.
        let landed: Vec<usize> = frames.iter().map(|&f| (f as usize).min(block - 1)).collect();
        for frame in &landed {
            prop_assert!(got.iter().any(|(start, _, _)| start == frame));
        }
        let mut delivered = Vec::new();
        for (start, _, places) in &got {
            for &p in places {
                prop_assert_eq!(landed[p as usize], *start);
                delivered.push(p);
            }
        }
        let expected: Vec<u32> = (0..frames.len() as u32).collect();
        prop_assert_eq!(delivered, expected);
    }
}
