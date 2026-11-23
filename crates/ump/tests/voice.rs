//! Reading and writing channel voice packets: MIDI 2.0's, which everything
//! above the exporters reads, and MIDI 1.0's, which the exporters translate.

use hapto_ump::{Attribute, Bank, ChannelMessage, ChannelVoice, Event, Midi1Message, Midi1Voice};
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

/// Checks that `words` reads as `message` on group 0, channel 0, and that
/// the message writes back to the same words.
fn reads_back(words: [u32; 4], message: ChannelMessage) {
    let expected = ChannelVoice {
        group: 0,
        channel: 0,
        message,
    };
    assert_eq!(ChannelVoice::decode(words), Some(expected), "{words:08x?}");
    assert_eq!(expected.words(), words, "{expected:?}");
    assert_eq!(Event::new(0, words).channel_voice(), Some(expected));
}

#[test]
fn known_midi_2_note_and_channel_packets_read_as_their_messages() {
    use ChannelMessage::{
        ChannelPressure, NoteOff, NoteOn, PitchBend, PolyPressure, ProgramChange,
    };
    // Group 2, channel 5, note 64, full velocity.
    let on = ChannelVoice {
        group: 2,
        channel: 5,
        message: NoteOn {
            note: 64,
            velocity: 0xFFFF,
            attribute: Attribute::NONE,
        },
    };
    assert_eq!(
        ChannelVoice::decode([0x4295_4000, 0xFFFF_0000, 0, 0]),
        Some(on)
    );
    assert_eq!(on.words(), [0x4295_4000, 0xFFFF_0000, 0, 0]);
    // A pitch 7.9 attribute is type 3.
    let pitch = Attribute {
        kind: 3,
        data: 0x8A00,
    };
    reads_back(
        [0x4080_3C03, 0x8000_8A00, 0, 0],
        NoteOff {
            note: 60,
            velocity: 0x8000,
            attribute: pitch,
        },
    );
    reads_back([0x40A0_3C00, 7, 0, 0], PolyPressure { note: 60, value: 7 });
    let bank = Some(Bank { msb: 1, lsb: 2 });
    reads_back(
        [0x40C0_0001, 0x0500_0102, 0, 0],
        ProgramChange { program: 5, bank },
    );
    reads_back([0x40D0_0000, 9, 0, 0], ChannelPressure { value: 9 });
    reads_back(
        [0x40E0_0000, 0x8000_0000, 0, 0],
        PitchBend { value: 0x8000_0000 },
    );
    // With the bank-valid bit clear there is no bank, whatever its fields
    // hold, and the packet goes back out with them cleared.
    let program = ChannelVoice {
        group: 0,
        channel: 0,
        message: ProgramChange {
            program: 5,
            bank: None,
        },
    };
    assert_eq!(
        ChannelVoice::decode([0x40C0_0000, 0x0500_0102, 0, 0]),
        Some(program)
    );
    assert_eq!(program.words(), [0x40C0_0000, 0x0500_0000, 0, 0]);
}

#[test]
fn known_midi_2_controller_packets_read_as_their_messages() {
    use ChannelMessage::{
        AssignableController, ControlChange, RegisteredController, RelativeAssignableController,
        RelativeRegisteredController,
    };
    reads_back(
        [0x40B0_0700, u32::MAX, 0, 0],
        ControlChange {
            controller: 7,
            value: u32::MAX,
        },
    );
    reads_back(
        [0x4020_0002, 0x8000_0000, 0, 0],
        RegisteredController {
            bank: 0,
            index: 2,
            value: 0x8000_0000,
        },
    );
    reads_back(
        [0x4030_0205, 1, 0, 0],
        AssignableController {
            bank: 2,
            index: 5,
            value: 1,
        },
    );
    // A relative change of -1, two's complement.
    reads_back(
        [0x4040_0001, u32::MAX, 0, 0],
        RelativeRegisteredController {
            bank: 0,
            index: 1,
            change: -1,
        },
    );
    reads_back(
        [0x4050_0001, 3, 0, 0],
        RelativeAssignableController {
            bank: 0,
            index: 1,
            change: 3,
        },
    );
}

#[test]
fn known_midi_2_per_note_packets_read_as_their_messages() {
    use ChannelMessage::{
        AssignablePerNoteController, PerNoteManagement, PerNotePitchBend,
        RegisteredPerNoteController,
    };
    // A per-note controller's index runs to 255.
    reads_back(
        [0x4000_3CFF, 4, 0, 0],
        RegisteredPerNoteController {
            note: 60,
            index: 255,
            value: 4,
        },
    );
    reads_back(
        [0x4010_3C01, 4, 0, 0],
        AssignablePerNoteController {
            note: 60,
            index: 1,
            value: 4,
        },
    );
    reads_back(
        [0x4060_3C00, 0x8000_0000, 0, 0],
        PerNotePitchBend {
            note: 60,
            value: 0x8000_0000,
        },
    );
    // Detach is bit 1 of the flags, reset bit 0.
    reads_back(
        [0x40F0_3C02, 0, 0, 0],
        PerNoteManagement {
            note: 60,
            detach: true,
            reset: false,
        },
    );
}

#[test]
fn known_midi_1_packets_read_as_their_messages() {
    let one = |group, channel, message| Midi1Voice {
        group,
        channel,
        message,
    };
    let cases = [
        (
            [0x2BB3_077F, 0, 0, 0],
            one(
                11,
                3,
                Midi1Message::ControlChange {
                    controller: 7,
                    value: 127,
                },
            ),
        ),
        (
            [0x2090_3C00, 0, 0, 0],
            one(
                0,
                0,
                Midi1Message::NoteOn {
                    note: 60,
                    velocity: 0,
                },
            ),
        ),
        (
            [0x2081_3C40, 0, 0, 0],
            one(
                0,
                1,
                Midi1Message::NoteOff {
                    note: 60,
                    velocity: 64,
                },
            ),
        ),
        (
            [0x20A0_3C10, 0, 0, 0],
            one(
                0,
                0,
                Midi1Message::PolyPressure {
                    note: 60,
                    value: 16,
                },
            ),
        ),
        (
            [0x20C0_0500, 0, 0, 0],
            one(0, 0, Midi1Message::ProgramChange { program: 5 }),
        ),
        (
            [0x20D0_2000, 0, 0, 0],
            one(0, 0, Midi1Message::ChannelPressure { value: 32 }),
        ),
        // Pitch bend sends its low 7 bits first: 0x2000 is at rest.
        (
            [0x20E0_0040, 0, 0, 0],
            one(0, 0, Midi1Message::PitchBend { value: 0x2000 }),
        ),
        (
            [0x20E0_7F7F, 0, 0, 0],
            one(0, 0, Midi1Message::PitchBend { value: 0x3FFF }),
        ),
    ];
    for (words, expected) in cases {
        assert_eq!(Midi1Voice::decode(words), Some(expected), "{words:08x?}");
        assert_eq!(expected.words(), words, "{expected:?}");
        assert_eq!(Event::new(0, words).midi1_voice(), Some(expected));
    }
}

#[test]
fn each_reader_reads_its_own_protocol_alone() {
    let midi1 = [0x2090_3C7F, 0, 0, 0];
    let midi2 = [0x4090_3C00, 0xFFFF_0000, 0, 0];
    assert_eq!(ChannelVoice::decode(midi1), None);
    assert_eq!(Midi1Voice::decode(midi2), None);
    // SysEx7, a system message and a utility message are neither.
    for words in [
        [0x3016_7E7F, 0x0D70_0201, 0, 0],
        [0x10F8_0000, 0, 0, 0],
        [0, 0, 0, 0],
    ] {
        assert_eq!(ChannelVoice::decode(words), None);
        assert_eq!(Midi1Voice::decode(words), None);
    }
    // MIDI 2.0 status 0x7 is reserved.
    assert_eq!(ChannelVoice::decode([0x4070_0000, 0, 0, 0]), None);
}

#[test]
fn fields_past_their_width_are_masked() {
    let voice = ChannelVoice {
        group: 0x1F,
        channel: 0x1F,
        message: ChannelMessage::ControlChange {
            controller: 200,
            value: 1,
        },
    };
    assert_eq!(voice.words(), [0x4FBF_4800, 1, 0, 0]);
    let one = Midi1Voice {
        group: 0,
        channel: 0,
        message: Midi1Message::PitchBend { value: 0xFFFF },
    };
    assert_eq!(one.words(), [0x20E0_7F7F, 0, 0, 0]);
}

fn seven() -> impl Strategy<Value = u8> {
    0_u8..128
}

fn midi2_message() -> impl Strategy<Value = ChannelMessage> {
    let attribute = (any::<u8>(), any::<u16>()).prop_map(|(kind, data)| Attribute { kind, data });
    let bank = prop::option::of((seven(), seven()).prop_map(|(msb, lsb)| Bank { msb, lsb }));
    prop_oneof![
        (seven(), any::<u16>(), attribute.clone()).prop_map(|(note, velocity, attribute)| {
            ChannelMessage::NoteOff {
                note,
                velocity,
                attribute,
            }
        }),
        (seven(), any::<u16>(), attribute).prop_map(|(note, velocity, attribute)| {
            ChannelMessage::NoteOn {
                note,
                velocity,
                attribute,
            }
        }),
        (seven(), any::<u32>())
            .prop_map(|(note, value)| ChannelMessage::PolyPressure { note, value }),
        (seven(), any::<u32>())
            .prop_map(|(controller, value)| ChannelMessage::ControlChange { controller, value }),
        (seven(), bank).prop_map(|(program, bank)| ChannelMessage::ProgramChange { program, bank }),
        any::<u32>().prop_map(|value| ChannelMessage::ChannelPressure { value }),
        any::<u32>().prop_map(|value| ChannelMessage::PitchBend { value }),
        (seven(), seven(), any::<u32>()).prop_map(|(bank, index, value)| {
            ChannelMessage::RegisteredController { bank, index, value }
        }),
        (seven(), seven(), any::<u32>()).prop_map(|(bank, index, value)| {
            ChannelMessage::AssignableController { bank, index, value }
        }),
        (seven(), seven(), any::<i32>()).prop_map(|(bank, index, change)| {
            ChannelMessage::RelativeRegisteredController {
                bank,
                index,
                change,
            }
        }),
        (seven(), seven(), any::<i32>()).prop_map(|(bank, index, change)| {
            ChannelMessage::RelativeAssignableController {
                bank,
                index,
                change,
            }
        }),
        (seven(), any::<u8>(), any::<u32>()).prop_map(|(note, index, value)| {
            ChannelMessage::RegisteredPerNoteController { note, index, value }
        }),
        (seven(), any::<u8>(), any::<u32>()).prop_map(|(note, index, value)| {
            ChannelMessage::AssignablePerNoteController { note, index, value }
        }),
        (seven(), any::<u32>())
            .prop_map(|(note, value)| ChannelMessage::PerNotePitchBend { note, value }),
        (seven(), any::<bool>(), any::<bool>()).prop_map(|(note, detach, reset)| {
            ChannelMessage::PerNoteManagement {
                note,
                detach,
                reset,
            }
        }),
    ]
}

fn midi1_message() -> impl Strategy<Value = Midi1Message> {
    prop_oneof![
        (seven(), seven()).prop_map(|(note, velocity)| Midi1Message::NoteOff { note, velocity }),
        (seven(), seven()).prop_map(|(note, velocity)| Midi1Message::NoteOn { note, velocity }),
        (seven(), seven()).prop_map(|(note, value)| Midi1Message::PolyPressure { note, value }),
        (seven(), seven())
            .prop_map(|(controller, value)| Midi1Message::ControlChange { controller, value }),
        seven().prop_map(|program| Midi1Message::ProgramChange { program }),
        seven().prop_map(|value| Midi1Message::ChannelPressure { value }),
        (0_u16..0x4000).prop_map(|value| Midi1Message::PitchBend { value }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "tests/voice.proptest-regressions",
        ))),
        cases: 2048,
        ..ProptestConfig::default()
    })]

    /// Every MIDI 2.0 message comes back from its packet as it went in.
    #[test]
    fn a_midi_2_message_reads_back_from_its_packet(
        group in 0_u8..16, channel in 0_u8..16, message in midi2_message(),
    ) {
        let voice = ChannelVoice { group, channel, message };
        prop_assert_eq!(ChannelVoice::decode(voice.words()), Some(voice));
    }

    /// Every MIDI 1.0 message comes back from its packet as it went in.
    #[test]
    fn a_midi_1_message_reads_back_from_its_packet(
        group in 0_u8..16, channel in 0_u8..16, message in midi1_message(),
    ) {
        let voice = Midi1Voice { group, channel, message };
        prop_assert_eq!(Midi1Voice::decode(voice.words()), Some(voice));
    }
}
