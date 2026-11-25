//! Tests decoding packets into messages:
//! - Every channel message's values.
//! - Notes and their ids, and a note's own messages.
//! - SysEx joined across packets.
//! - Each message's line.

use hapto_midi::{Centred, Channel, Decoder, Group, Message, NoteId, Program, controller};
use hapto_ump::SysExForm::{self, Complete, Continue, End, Start};
use hapto_ump::{Attribute, Event, Origin, SysEx7, scale};
use hapto_ump::{EventList, Midi1Message, Midi1Voice, Upgrade};
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

fn packet(words: [u32; 2]) -> Event {
    Event::new(0, [words[0], words[1], 0, 0])
}

fn decode(decoder: &mut Decoder, words: [u32; 2]) -> Vec<Message<'_>> {
    decoder.decode(&packet(words)).collect()
}

fn one(words: [u32; 2]) -> String {
    let mut decoder = Decoder::new();
    let messages = decode(&mut decoder, words);
    assert_eq!(messages.len(), 1, "{words:08X?}");
    messages[0].to_string()
}

const FIRST: (Group, Channel) = (Group::FIRST, Channel::FIRST);

#[test]
fn every_channel_message_decodes_to_its_values() {
    let mut decoder = Decoder::new();
    let first = |messages: Vec<Message<'_>>| messages.first().copied().map(|m| m.to_string());
    let cases: [([u32; 2], Message<'_>); 7] = [
        (
            [0x40B0_0700, u32::MAX],
            Message::Control {
                group: FIRST.0,
                channel: FIRST.1,
                controller: controller::number::VOLUME,
                value: 1.0,
            },
        ),
        (
            [0x40E0_0000, 0x8000_0000],
            Message::Bend {
                group: FIRST.0,
                channel: FIRST.1,
                value: Centred::CENTRE,
            },
        ),
        (
            [0x40D0_0000, 0],
            Message::Pressure {
                group: FIRST.0,
                channel: FIRST.1,
                value: 0.0,
            },
        ),
        (
            [0x40C0_0001, 0x0500_0102],
            Message::Program {
                group: FIRST.0,
                channel: FIRST.1,
                program: Program::number(6),
                bank: Some(1 << 7 | 2),
            },
        ),
        (
            [0x40C0_0000, 0x0500_0000],
            Message::Program {
                group: FIRST.0,
                channel: FIRST.1,
                program: Program::number(6),
                bank: None,
            },
        ),
        (
            [0x4020_0102, u32::MAX],
            Message::Registered {
                group: FIRST.0,
                channel: FIRST.1,
                bank: 1,
                index: 2,
                value: 1.0,
            },
        ),
        (
            [0x4535_0205, 0],
            Message::Assignable {
                group: Group::number(6),
                channel: Channel::number(6),
                bank: 2,
                index: 5,
                value: 0.0,
            },
        ),
    ];
    for (words, expected) in cases {
        let messages = decode(&mut decoder, words);
        assert_eq!(messages, [expected], "{words:08X?}");
        assert_eq!(first(messages), Some(expected.to_string()));
    }
}

/// Returns MIDI 1.0 control changes from a device, upgraded as the engine
/// does.
fn upgraded(sent: &[(u8, u8)]) -> Vec<Event> {
    let events: Vec<Event> = sent
        .iter()
        .map(|&(controller, value)| {
            let message = Midi1Message::ControlChange { controller, value };
            Midi1Voice {
                group: 0,
                channel: 0,
                message,
            }
            .event(0)
        })
        .collect();
    let mut out = EventList::with_capacity(8);
    Upgrade::new().block(&events, 0, &mut out);
    out.as_slice().to_vec()
}

#[test]
fn registered_parameter_0_is_the_bend_range_in_semitones() {
    // MIDI 1.0's RPN 0, 12 semitones and 50 cents.
    let sent = upgraded(&[(101, 0), (100, 0), (6, 12), (38, 50)]);
    // The upgrade makes the four controllers one registered controller.
    assert_eq!(sent.len(), 1);
    let mut decoder = Decoder::new();
    let messages: Vec<Message<'_>> = decoder.decode(&sent[0]).collect();
    assert_eq!(
        messages,
        [Message::BendRange {
            group: FIRST.0,
            channel: FIRST.1,
            semitones: 12.5,
        }]
    );
}

#[test]
fn a_midi_1_controller_upgraded_lands_on_0_the_centre_and_1_exactly() {
    for (sent, expected) in [(0, 0.0), (64, 0.5), (127, 1.0)] {
        let mut decoder = Decoder::new();
        let event = upgraded(&[(1, sent)])[0];
        let message = decoder.decode(&event).next();
        assert!(
            matches!(message, Some(Message::Control { value, .. }) if value == expected),
            "{sent}: {message:?}"
        );
    }
}

fn on(key: u32, velocity: u16) -> [u32; 2] {
    [0x4090_0000 | key << 8, u32::from(velocity) << 16]
}

fn off(key: u32) -> [u32; 2] {
    [0x4080_0000 | key << 8, 0x8000_0000]
}

fn note_id(message: &Message<'_>) -> NoteId {
    match message {
        Message::NoteOn(note) => note.id,
        Message::NoteOff(off) => off.id,
        other => panic!("not a note: {other:?}"),
    }
}

#[test]
fn each_note_has_an_id_of_its_own_that_its_note_off_names() {
    let mut decoder = Decoder::new();
    let first = note_id(&decode(&mut decoder, on(60, 0xFFFF))[0]);
    let second = note_id(&decode(&mut decoder, on(64, 0xFFFF))[0]);
    assert_ne!(first, second);
    assert!(!first.is_hosts());
    assert_eq!(decoder.sounding(), 2);
    let ended: Vec<NoteId> = decode(&mut decoder, off(64)).iter().map(note_id).collect();
    assert_eq!(ended, [second]);
    let ended: Vec<NoteId> = decode(&mut decoder, off(60)).iter().map(note_id).collect();
    assert_eq!((ended, decoder.sounding()), (vec![first], 0));
}

#[test]
fn a_note_off_ends_every_note_on_its_key_and_one_with_none_still_comes() {
    let mut decoder = Decoder::new();
    let a = note_id(&decode(&mut decoder, on(60, 0xFFFF))[0]);
    let b = note_id(&decode(&mut decoder, on(60, 0x8000))[0]);
    let ended: Vec<NoteId> = decode(&mut decoder, off(60)).iter().map(note_id).collect();
    assert_eq!(ended, [a, b]);
    // With no note on the key, a note-off still comes, under an id that no
    // note has, for a plugin that ends notes by key.
    let orphan = decode(&mut decoder, off(60));
    assert_eq!(orphan.len(), 1);
    let id = note_id(&orphan[0]);
    assert!(id != a && id != b);
}

#[test]
fn a_notes_own_message_names_its_key_and_the_newest_note_on_it() {
    let mut decoder = Decoder::new();
    let pressure = [0x40A0_3C00, u32::MAX];
    let before = decode(&mut decoder, pressure);
    assert!(matches!(before[0], Message::NotePressure { note, value: 1.0 } if note.id.is_none()));
    decode(&mut decoder, on(60, 0xFFFF));
    let newer = note_id(&decode(&mut decoder, on(60, 0xFFFF))[0]);
    let during = decode(&mut decoder, pressure);
    assert!(matches!(during[0], Message::NotePressure { note, .. } if note.id == Some(newer)));
    // Once detached, the key's notes take no more per-note messages.
    decode(&mut decoder, [0x40F0_3C02, 0]);
    let after = decode(&mut decoder, pressure);
    assert!(matches!(after[0], Message::NotePressure { note, .. } if note.id.is_none()));
    // A new note on the key takes them again.
    let next = note_id(&decode(&mut decoder, on(60, 0xFFFF))[0]);
    let again = decode(&mut decoder, pressure);
    assert!(matches!(again[0], Message::NotePressure { note, .. } if note.id == Some(next)));
}

/// Returns `words` as a packet with the host's note id `id`, as in VST3.
fn named(words: [u32; 2], id: u32) -> Event {
    Event {
        origin: Origin::Note { id },
        ..packet(words)
    }
}

#[test]
fn a_note_its_host_named_keeps_that_id_and_its_note_off_ends_it_alone() {
    let mut decoder = Decoder::new();
    let ids: Vec<NoteId> = [named(on(60, 0xFFFF), 7), named(on(60, 0xFFFF), 9)]
        .iter()
        .map(|event| note_id(&decoder.decode(event).next().expect("a note-on")))
        .collect();
    assert_eq!(ids, [NoteId::hosts(7), NoteId::hosts(9)]);
    assert!(ids[0].is_hosts());
    // With two notes on one key, the note-off that names the first ends it
    // alone.
    let ended: Vec<NoteId> = decoder
        .decode(&named(off(60), 7))
        .map(|m| note_id(&m))
        .collect();
    assert_eq!((ended, decoder.sounding()), (vec![NoteId::hosts(7)], 1));
    // A note-off that names a note the decoder doesn't hold comes under the
    // host's id alone, and the other note on the key sounds on until its own
    // note-off ends it.
    let ended: Vec<NoteId> = decoder
        .decode(&named(off(60), 7))
        .map(|m| note_id(&m))
        .collect();
    assert_eq!((ended, decoder.sounding()), (vec![NoteId::hosts(7)], 1));
    let ended: Vec<NoteId> = decoder
        .decode(&named(off(60), 9))
        .map(|m| note_id(&m))
        .collect();
    assert_eq!((ended, decoder.sounding()), (vec![NoteId::hosts(9)], 0));
}

#[test]
fn a_note_past_a_full_table_takes_the_oldest_notes_place() {
    let mut decoder = Decoder::with_room(2, 0);
    let ids: Vec<NoteId> = [60, 62, 64]
        .map(|key| note_id(&decode(&mut decoder, on(key, 0xFFFF))[0]))
        .to_vec();
    assert_eq!((decoder.sounding(), decoder.dropped()), (2, 1));
    // The table holds the newest notes, and their note-offs name them.
    let ended = note_id(&decode(&mut decoder, off(64))[0]);
    assert_eq!(ended, ids[2]);
    // The oldest note's note-off comes under an id that no note has, for a
    // plugin that ends notes by key.
    let ended = note_id(&decode(&mut decoder, off(60))[0]);
    assert!(!ids.contains(&ended));
    assert_eq!(decoder.sounding(), 1);
    decoder.reset();
    assert_eq!(decoder.sounding(), 0);
}

#[test]
fn all_notes_off_ends_its_channels_notes_in_the_table_and_no_others() {
    let mut decoder = Decoder::new();
    decode(&mut decoder, on(60, 0xFFFF));
    decode(&mut decoder, on(64, 0xFFFF));
    // A note on the second channel.
    decode(&mut decoder, [0x4091_3C00, 0xFFFF_0000]);
    assert_eq!(decoder.sounding(), 3);
    let all_off = decode(&mut decoder, [0x40B0_7B00, 0]);
    assert!(matches!(
        all_off[0],
        Message::Control {
            controller: 123,
            ..
        }
    ));
    assert_eq!(decoder.sounding(), 1, "the second channel's note sounds on");
}

#[test]
fn a_notes_own_message_its_host_named_finds_that_note_and_not_the_newest() {
    let mut decoder = Decoder::new();
    decoder.decode(&named(on(60, 0xFFFF), 7)).count();
    decoder.decode(&named(on(60, 0xFFFF), 9)).count();
    let pressure = [0x40A0_3C00, u32::MAX];
    let older = decoder.decode(&named(pressure, 7)).next();
    assert!(matches!(
        older,
        Some(Message::NotePressure { note, .. }) if note.id == Some(NoteId::hosts(7))
    ));
    // A host's id for a note that isn't sounding names no note.
    let gone = decoder.decode(&named(pressure, 3)).next();
    assert!(matches!(
        gone,
        Some(Message::NotePressure { note, .. }) if note.id.is_none()
    ));
}

#[test]
fn a_notes_pitch_comes_with_it_or_on_its_own() {
    let mut decoder = Decoder::new();
    // A note-on for A4 with the pitch attribute at 69.5.
    let with_pitch = [0x4090_4503, 0xFFFF_0000 | (69 * 512 + 256)];
    let Message::NoteOn(note) = decode(&mut decoder, with_pitch)[0] else {
        panic!("a note-on");
    };
    assert_eq!(note.pitch().semitones(), 69.5);
    assert_eq!(note.attribute(), Some(Attribute::pitch(69.5)));
    // Registered per-note controller 3 carries 7 bits of key and 25 of
    // fraction.
    let quarter = [0x4000_4503, 60 << 25 | 1 << 23];
    let messages = decode(&mut decoder, quarter);
    assert!(matches!(messages[0], Message::NotePitch { pitch, .. } if pitch.semitones() == 60.25));
    // Another per-note controller keeps its number, such as brightness (74).
    let bright = decode(&mut decoder, [0x4000_454A, u32::MAX]);
    assert!(matches!(
        bright[0],
        Message::NoteControl {
            controller: controller::number::BRIGHTNESS,
            value: 1.0,
            ..
        }
    ));
}

#[test]
fn midi_1_reads_as_nothing_since_it_is_upgraded_before() {
    let mut decoder = Decoder::new();
    assert_eq!(decode(&mut decoder, [0x2090_3C7F, 0]), []);
    assert_eq!(decode(&mut decoder, [0x10F8_0000, 0]), []);
}

/// Returns a 7-bit SysEx packet of `form` on `group`, carrying `bytes`.
fn sysex(group: u8, form: SysExForm, bytes: &[u8]) -> [u32; 2] {
    let [first, second, ..] = SysEx7::new(group, form, bytes).words();
    [first, second]
}

fn data(messages: &[Message<'_>]) -> Vec<Vec<u8>> {
    messages
        .iter()
        .filter_map(|m| match m {
            Message::SysEx { data, .. } => Some(data.to_vec()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_sysex_comes_whole_once_its_last_packet_does() {
    let mut decoder = Decoder::new();
    let identity = decode(&mut decoder, sysex(0, Complete, &[0x7E, 0x7F, 0x06, 0x01]));
    assert_eq!(data(&identity), [vec![0x7E, 0x7F, 0x06, 0x01]]);
    assert_eq!(
        decode(&mut decoder, sysex(0, Start, &[1, 2, 3, 4, 5, 6])),
        []
    );
    // A clock inside it changes nothing.
    assert_eq!(decode(&mut decoder, [0x10F8_0000, 0]), []);
    assert_eq!(decode(&mut decoder, sysex(0, Continue, &[7, 8])), []);
    let whole = decode(&mut decoder, sysex(0, End, &[9]));
    assert_eq!(data(&whole), [(1..=9).collect::<Vec<u8>>()]);
    assert_eq!(decoder.dropped(), 0);
}

#[test]
fn a_sysex_too_long_cut_short_or_begun_elsewhere_is_dropped_and_counted() {
    let mut decoder = Decoder::with_room(8, 4);
    decode(&mut decoder, sysex(0, Start, &[1, 2, 3]));
    assert_eq!(
        decode(&mut decoder, sysex(0, End, &[4, 5])),
        [],
        "past the buffer"
    );
    assert_eq!(decoder.dropped(), 1);
    // A note on the SysEx's group cuts it short.
    decode(&mut decoder, sysex(0, Start, &[1]));
    decode(&mut decoder, on(60, 0xFFFF));
    assert_eq!(decode(&mut decoder, sysex(0, End, &[2])), []);
    assert_eq!(decoder.dropped(), 2);
    // A SysEx begun on another group while one is under way is dropped.
    decode(&mut decoder, sysex(0, Start, &[1]));
    assert_eq!(decode(&mut decoder, sysex(1, Complete, &[9])), []);
    let whole = decode(&mut decoder, sysex(0, End, &[2]));
    assert_eq!((data(&whole), decoder.dropped()), (vec![vec![1, 2]], 3));
}

#[test]
fn each_message_is_one_line_in_musicians_numbers() {
    let lines = [
        (
            on(60, 0xFFFF),
            "note on C4, velocity 1.000, channel 1, group 1, note 0",
        ),
        (
            [0x40B9_4000, 0],
            "controller 64 (Sustain Pedal) 0.000, channel 10, group 1",
        ),
        (
            [0x40E0_0000, u32::MAX],
            "pitch bend +1.000, channel 1, group 1",
        ),
        ([0x40C0_0001, 0], "program 1, bank 0, channel 1, group 1"),
        (
            [0x40A0_3D00, 0],
            "pressure 0.000 for C#4, channel 1, group 1, no note",
        ),
        (
            sysex(2, Complete, &[0x7E, 0x7F]),
            "SysEx of 2 bytes: 7E 7F, group 3",
        ),
    ];
    for (words, line) in lines {
        assert_eq!(one(words), line);
    }
    let mut decoder = Decoder::new();
    decode(&mut decoder, on(60, 0xFFFF));
    let ended = decode(&mut decoder, off(60));
    assert_eq!(
        ended[0].to_string(),
        "note off C4, velocity 0.500, channel 1, group 1, note 0"
    );
}

#[test]
fn a_bend_from_midi_1_reads_centred() {
    let mut decoder = Decoder::new();
    let rest = scale::up_14_to_32(0x2000);
    let messages = decode(&mut decoder, [0x40E0_0000, rest]);
    assert!(matches!(messages[0], Message::Bend { value, .. } if value == Centred::CENTRE));
}

/// A key pressed or released, for the model below.
#[derive(Debug, Clone, Copy)]
enum Played {
    On(u8),
    Off(u8),
}

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "tests/decode.proptest-regressions",
        ))),
        cases: 512,
        ..ProptestConfig::default()
    })]

    /// However keys are pressed and released, a note-off ends exactly the
    /// notes that are sounding on its key, in the order that they began, and
    /// every id is the note's own.
    #[test]
    fn a_note_off_ends_the_notes_its_key_has(
        played in prop::collection::vec(
            prop_oneof![(0_u8..4).prop_map(Played::On), (0_u8..4).prop_map(Played::Off)],
            0..64,
        ),
    ) {
        let mut decoder = Decoder::new();
        let mut model: Vec<(u8, NoteId)> = Vec::new();
        for step in played {
            match step {
                Played::On(key) => {
                    let messages = decode(&mut decoder, on(u32::from(key), 0xFFFF));
                    let id = note_id(&messages[0]);
                    prop_assert!(model.iter().all(|&(_, other)| other != id));
                    model.push((key, id));
                }
                Played::Off(key) => {
                    let ended: Vec<NoteId> =
                        decode(&mut decoder, off(u32::from(key))).iter().map(note_id).collect();
                    let expected: Vec<NoteId> =
                        model.iter().filter(|(k, _)| *k == key).map(|&(_, id)| id).collect();
                    if expected.is_empty() {
                        prop_assert_eq!(ended.len(), 1);
                    } else {
                        prop_assert_eq!(ended, expected);
                    }
                    model.retain(|(k, _)| *k != key);
                }
            }
            prop_assert_eq!(decoder.sounding(), model.len());
        }
    }
}
