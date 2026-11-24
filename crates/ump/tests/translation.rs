//! Translating between MIDI 1.0 and MIDI 2.0 by the Default Translation in
//! appendix D of the UMP and MIDI 2.0 Protocol specification: the upgrade
//! an exporter makes on the way in, the downgrade on the way out, and the
//! round trip that must give back what MIDI 1.0 sent.

use hapto_ump::{
    Attribute, Bank, ChannelMessage, ChannelVoice, Downgrade, Event, EventList, Midi1Message,
    Midi1Voice, Origin, Upgrade, scale,
};

const LAST_FRAME: u32 = 63;

fn one(message: Midi1Message) -> Midi1Voice {
    Midi1Voice {
        group: 0,
        channel: 0,
        message,
    }
}

fn cc(controller: u8, value: u8) -> Midi1Voice {
    one(Midi1Message::ControlChange { controller, value })
}

/// Upgrades `sent`, each a MIDI 1.0 message at its frame, as one block, and
/// returns what came out with each one's frame.
fn upgrade(upgrade: &mut Upgrade, sent: &[(u32, Midi1Voice)]) -> Vec<(u32, ChannelMessage)> {
    let input: Vec<Event> = sent
        .iter()
        .map(|(frame, voice)| voice.event(*frame))
        .collect();
    let mut out = EventList::with_capacity(64);
    upgrade.block(&input, LAST_FRAME, &mut out);
    out.as_slice()
        .iter()
        .map(|event| {
            assert!(
                matches!(event.origin, Origin::Midi1 | Origin::Midi1HighByte),
                "every packet it makes says so"
            );
            let voice = event.channel_voice().expect("a MIDI 2.0 message");
            (event.offset, voice.message)
        })
        .collect()
}

fn assignable(bank: u8, index: u8, value: u16) -> ChannelMessage {
    ChannelMessage::AssignableController {
        bank,
        index,
        value: scale::up_14_to_32(value),
    }
}

#[test]
fn a_note_on_at_velocity_0_is_a_note_off_at_the_centre() {
    let sent = [
        (
            0,
            one(Midi1Message::NoteOn {
                note: 60,
                velocity: 0,
            }),
        ),
        (
            1,
            one(Midi1Message::NoteOn {
                note: 60,
                velocity: 1,
            }),
        ),
    ];
    let none = Attribute::NONE;
    assert_eq!(
        upgrade(&mut Upgrade::new(), &sent),
        [
            (
                0,
                ChannelMessage::NoteOff {
                    note: 60,
                    velocity: 0x8000,
                    attribute: none
                }
            ),
            // D.3.1: a velocity of 1 is 0x0200.
            (
                1,
                ChannelMessage::NoteOn {
                    note: 60,
                    velocity: 0x0200,
                    attribute: none
                }
            ),
        ]
    );
}

#[test]
fn a_fourteen_bit_controller_pair_stays_two_controllers() {
    let out = upgrade(&mut Upgrade::new(), &[(0, cc(1, 64)), (0, cc(33, 5))]);
    assert_eq!(
        out,
        [
            (
                0,
                ChannelMessage::ControlChange {
                    controller: 1,
                    value: 0x8000_0000
                }
            ),
            (
                0,
                ChannelMessage::ControlChange {
                    controller: 33,
                    value: scale::up_7_to_32(5)
                }
            ),
        ]
    );
}

#[test]
fn an_nrpn_goes_out_when_its_low_byte_arrives() {
    let sent = [
        (0, cc(99, 2)),
        (0, cc(98, 5)),
        (4, cc(6, 100)),
        (9, cc(38, 3)),
    ];
    assert_eq!(
        upgrade(&mut Upgrade::new(), &sent),
        [(9, assignable(2, 5, 100 << 7 | 3))]
    );
}

#[test]
fn a_high_byte_alone_goes_out_at_the_end_of_its_block() {
    let mut up = Upgrade::new();
    let sent = [(0, cc(99, 2)), (0, cc(98, 5)), (4, cc(6, 100))];
    assert_eq!(
        upgrade(&mut up, &sent),
        [(LAST_FRAME, assignable(2, 5, 100 << 7))]
    );
    // A low byte in the next block refines it.
    assert_eq!(
        upgrade(&mut up, &[(2, cc(38, 3))]),
        [(2, assignable(2, 5, 100 << 7 | 3))]
    );
}

#[test]
fn a_high_byte_sent_alone_is_marked_apart_from_a_low_byte_of_0() {
    // The same value either way, but a reader of the width it was sent at
    // must tell a low byte that never came from one of 0.
    let mut up = Upgrade::new();
    let mut out = EventList::with_capacity(4);
    let high = [cc(99, 2).event(0), cc(98, 5).event(0), cc(6, 64).event(1)];
    up.block(&high, LAST_FRAME, &mut out);
    up.block(&[cc(38, 0).event(0)], LAST_FRAME, &mut out);
    let [alone, refined] = out.as_slice() else {
        panic!("two messages: {:?}", out.as_slice());
    };
    assert_eq!(
        [
            (alone.offset, alone.origin),
            (refined.offset, refined.origin)
        ],
        [(LAST_FRAME, Origin::Midi1HighByte), (0, Origin::Midi1)]
    );
    assert_eq!(alone.words, refined.words);
}

#[test]
fn another_high_byte_or_a_new_address_ends_the_one_held() {
    let sent = [
        (0, cc(99, 2)),
        (0, cc(98, 5)),
        (1, cc(6, 10)),
        // A second high byte sends the first.
        (2, cc(6, 20)),
        // A new address sends the second, under the old address.
        (3, cc(99, 7)),
        (3, cc(98, 1)),
        (4, cc(6, 30)),
    ];
    assert_eq!(
        upgrade(&mut Upgrade::new(), &sent),
        [
            (2, assignable(2, 5, 10 << 7)),
            (3, assignable(2, 5, 20 << 7)),
            (LAST_FRAME, assignable(7, 1, 30 << 7)),
        ]
    );
}

#[test]
fn an_rpn_is_a_registered_controller() {
    let sent = [
        (0, cc(101, 0)),
        (0, cc(100, 0)),
        (0, cc(6, 12)),
        (0, cc(38, 0)),
    ];
    // Pitch bend sensitivity, 12 semitones.
    assert_eq!(
        upgrade(&mut Upgrade::new(), &sent),
        [(
            0,
            ChannelMessage::RegisteredController {
                bank: 0,
                index: 0,
                value: scale::up_14_to_32(12 << 7)
            }
        )]
    );
}

#[test]
fn an_address_reads_in_either_byte_order() {
    // JUCE's generator sends the low byte first.
    let sent = [
        (0, cc(98, 5)),
        (0, cc(99, 2)),
        (0, cc(6, 1)),
        (0, cc(38, 1)),
    ];
    assert_eq!(
        upgrade(&mut Upgrade::new(), &sent),
        [(0, assignable(2, 5, 1 << 7 | 1))]
    );
}

#[test]
fn a_low_byte_sent_first_waits_for_its_high_byte() {
    let sent = [
        (0, cc(99, 2)),
        (0, cc(98, 5)),
        (1, cc(38, 3)),
        (2, cc(6, 100)),
    ];
    assert_eq!(
        upgrade(&mut Upgrade::new(), &sent),
        [(2, assignable(2, 5, 100 << 7 | 3))]
    );
}

#[test]
fn an_address_sent_again_keeps_its_high_byte_for_the_low_byte() {
    // D.3.3 holds the latest high byte, and a low byte goes out with it.
    let mut up = Upgrade::new();
    let sent = [
        (0, cc(99, 2)),
        (0, cc(98, 5)),
        (1, cc(6, 100)),
        // A sender that names the address before every data byte.
        (2, cc(99, 2)),
        (2, cc(98, 5)),
        (3, cc(38, 3)),
    ];
    assert_eq!(
        upgrade(&mut up, &sent),
        [
            (2, assignable(2, 5, 100 << 7)),
            (3, assignable(2, 5, 100 << 7 | 3)),
        ]
    );
    // A move of the low byte alone, a block later.
    let fine = [(0, cc(99, 2)), (0, cc(98, 5)), (1, cc(38, 4))];
    assert_eq!(
        upgrade(&mut up, &fine),
        [(1, assignable(2, 5, 100 << 7 | 4))]
    );
}

#[test]
fn a_low_byte_never_takes_another_numbers_high_byte() {
    let sent = [
        (0, cc(99, 2)),
        (0, cc(98, 5)),
        (0, cc(6, 100)),
        (0, cc(38, 0)),
        // Another number's low byte waits for that number's own high byte.
        (1, cc(99, 7)),
        (1, cc(98, 1)),
        (2, cc(38, 3)),
        (3, cc(6, 30)),
    ];
    assert_eq!(
        upgrade(&mut Upgrade::new(), &sent),
        [
            (0, assignable(2, 5, 100 << 7)),
            (3, assignable(7, 1, 30 << 7 | 3)),
        ]
    );
}

#[test]
fn the_null_number_and_a_lone_data_entry_send_nothing() {
    let sent = [
        (0, cc(99, 127)),
        (0, cc(98, 127)),
        (1, cc(6, 10)),
        (2, cc(38, 1)),
    ];
    assert_eq!(upgrade(&mut Upgrade::new(), &sent), []);
    assert_eq!(
        upgrade(&mut Upgrade::new(), &[(0, cc(6, 10)), (0, cc(38, 1))]),
        []
    );
}

#[test]
fn reset_all_controllers_sends_what_is_held_then_forgets_the_address() {
    let sent = [
        (0, cc(99, 2)),
        (0, cc(98, 5)),
        (1, cc(6, 10)),
        (2, cc(121, 0)),
        (3, cc(6, 20)),
    ];
    assert_eq!(
        upgrade(&mut Upgrade::new(), &sent),
        [
            (2, assignable(2, 5, 10 << 7)),
            (
                2,
                ChannelMessage::ControlChange {
                    controller: 121,
                    value: 0
                }
            ),
        ]
    );
}

#[test]
fn increment_and_decrement_are_controllers_of_their_own() {
    let sent = [(0, cc(96, 1)), (0, cc(97, 1))];
    let up = scale::up_7_to_32(1);
    assert_eq!(
        upgrade(&mut Upgrade::new(), &sent),
        [
            (
                0,
                ChannelMessage::ControlChange {
                    controller: 96,
                    value: up
                }
            ),
            (
                0,
                ChannelMessage::ControlChange {
                    controller: 97,
                    value: up
                }
            ),
        ]
    );
    // A high byte held goes out first, so a MIDI 1.0 receiver steps the
    // value it sets rather than the one before.
    let sent = [
        (0, cc(99, 2)),
        (0, cc(98, 5)),
        (1, cc(6, 100)),
        (2, cc(96, 1)),
    ];
    assert_eq!(
        upgrade(&mut Upgrade::new(), &sent),
        [
            (2, assignable(2, 5, 100 << 7)),
            (
                2,
                ChannelMessage::ControlChange {
                    controller: 96,
                    value: up
                }
            ),
        ]
    );
}

#[test]
fn bank_select_is_held_for_the_program_change_that_carries_it() {
    let mut up = Upgrade::new();
    let program = |program| one(Midi1Message::ProgramChange { program });
    assert_eq!(
        upgrade(&mut up, &[(0, program(5))]),
        [(
            0,
            ChannelMessage::ProgramChange {
                program: 5,
                bank: None
            }
        )]
    );
    // Controllers 0 and 32 alone translate to nothing.
    assert_eq!(upgrade(&mut up, &[(0, cc(0, 1)), (0, cc(32, 2))]), []);
    let bank = Some(Bank { msb: 1, lsb: 2 });
    assert_eq!(
        upgrade(&mut up, &[(3, program(6))]),
        [(3, ChannelMessage::ProgramChange { program: 6, bank })]
    );
    // The bank stays selected for the next program change.
    assert_eq!(
        upgrade(&mut up, &[(0, program(7))]),
        [(0, ChannelMessage::ProgramChange { program: 7, bank })]
    );
}

#[test]
fn a_bank_byte_sent_alone_selects_a_bank_with_its_partner_at_0() {
    // D.3.4: a program change carries the bank select values current.
    let program = one(Midi1Message::ProgramChange { program: 5 });
    for (byte, bank) in [
        (cc(0, 1), Bank { msb: 1, lsb: 0 }),
        (cc(32, 2), Bank { msb: 0, lsb: 2 }),
    ] {
        assert_eq!(
            upgrade(&mut Upgrade::new(), &[(0, byte), (0, program)]),
            [(
                0,
                ChannelMessage::ProgramChange {
                    program: 5,
                    bank: Some(bank)
                }
            )]
        );
    }
}

#[test]
fn groups_and_channels_keep_their_own_state() {
    let on = |group, channel, voice: Midi1Voice| Midi1Voice {
        group,
        channel,
        ..voice
    };
    let sent = [
        (0, on(0, 0, cc(99, 2))),
        (0, on(0, 0, cc(98, 5))),
        // The same data entry on another channel has no address.
        (1, on(0, 1, cc(6, 10))),
        (2, on(3, 0, cc(6, 10))),
        (3, on(0, 0, cc(6, 10))),
    ];
    assert_eq!(
        upgrade(&mut Upgrade::new(), &sent),
        [(LAST_FRAME, assignable(2, 5, 10 << 7))]
    );
}

#[test]
fn every_channel_still_holding_a_high_byte_sends_it_at_the_end_of_the_block() {
    let on = |group, channel, voice: Midi1Voice| Midi1Voice {
        group,
        channel,
        ..voice
    };
    let mut sent = Vec::new();
    for (frame, (group, channel, high)) in (0..).zip([(3, 9, 30), (0, 15, 20), (0, 4, 10)]) {
        sent.extend([cc(99, 2), cc(98, 5), cc(6, high)].map(|cc| (frame, on(group, channel, cc))));
    }
    // The low byte completes one; the other two go out at the end, group by
    // group and channel by channel.
    sent.push((5, on(0, 15, cc(38, 1))));
    assert_eq!(
        upgrade(&mut Upgrade::new(), &sent),
        [
            (5, assignable(2, 5, 20 << 7 | 1)),
            (LAST_FRAME, assignable(2, 5, 10 << 7)),
            (LAST_FRAME, assignable(2, 5, 30 << 7)),
        ]
    );
}

#[test]
fn other_packets_pass_as_they_are() {
    let midi2 = Event::new(0, [0x40B0_0700, 1, 0, 0]);
    let sysex = Event::new(1, [0x3016_7E7F, 0x0D70_0201, 0, 0]);
    let mut out = EventList::with_capacity(4);
    Upgrade::new().block(&[midi2, sysex], LAST_FRAME, &mut out);
    assert_eq!(out.as_slice(), [midi2, sysex]);
}

/// Downgrades `messages`, each a MIDI 2.0 message at frame 0, and returns
/// what came out.
fn downgrade(downgrade: &mut Downgrade, messages: &[ChannelMessage]) -> Vec<Midi1Message> {
    let input: Vec<Event> = messages
        .iter()
        .map(|&message| {
            ChannelVoice {
                group: 0,
                channel: 0,
                message,
            }
            .event(0)
        })
        .collect();
    let mut out = EventList::with_capacity(64);
    downgrade.block(&input, &mut out);
    out.as_slice()
        .iter()
        .map(|event| event.midi1_voice().expect("a MIDI 1.0 message").message)
        .collect()
}

#[test]
fn a_note_on_that_would_cut_to_0_goes_out_at_velocity_1() {
    let none = Attribute::NONE;
    let sent = downgrade(
        &mut Downgrade::new(),
        &[ChannelMessage::NoteOn {
            note: 60,
            velocity: 0x01FF,
            attribute: none,
        }],
    );
    assert_eq!(
        sent,
        [Midi1Message::NoteOn {
            note: 60,
            velocity: 1
        }]
    );
}

#[test]
fn a_controller_becomes_its_four_and_a_program_its_bank_first() {
    let mut down = Downgrade::new();
    let cc = |controller, value| Midi1Message::ControlChange { controller, value };
    let rpn = ChannelMessage::RegisteredController {
        bank: 0,
        index: 0,
        value: scale::up_14_to_32(12 << 7 | 5),
    };
    assert_eq!(
        downgrade(&mut down, &[rpn]),
        [cc(101, 0), cc(100, 0), cc(6, 12), cc(38, 5)]
    );
    let nrpn = ChannelMessage::AssignableController {
        bank: 2,
        index: 5,
        value: 0,
    };
    assert_eq!(
        downgrade(&mut down, &[nrpn]),
        [cc(99, 2), cc(98, 5), cc(6, 0), cc(38, 0)]
    );
    let program = ChannelMessage::ProgramChange {
        program: 6,
        bank: Some(Bank { msb: 1, lsb: 2 }),
    };
    assert_eq!(
        downgrade(&mut down, &[program]),
        [
            cc(0, 1),
            cc(32, 2),
            Midi1Message::ProgramChange { program: 6 }
        ]
    );
}

#[test]
fn what_midi_1_cannot_carry_is_dropped_and_counted() {
    let mut down = Downgrade::new();
    let lost = [
        ChannelMessage::RelativeRegisteredController {
            bank: 0,
            index: 0,
            change: 1,
        },
        ChannelMessage::RelativeAssignableController {
            bank: 0,
            index: 0,
            change: 1,
        },
        ChannelMessage::RegisteredPerNoteController {
            note: 60,
            index: 3,
            value: 1,
        },
        ChannelMessage::AssignablePerNoteController {
            note: 60,
            index: 3,
            value: 1,
        },
        ChannelMessage::PerNotePitchBend { note: 60, value: 1 },
        ChannelMessage::PerNoteManagement {
            note: 60,
            detach: true,
            reset: true,
        },
    ];
    assert_eq!(downgrade(&mut down, &lost), []);
    assert_eq!(down.lost(), 6);
    // A note's attribute is lost, and the note still goes out.
    let pitched = ChannelMessage::NoteOn {
        note: 60,
        velocity: 0xFFFF,
        attribute: Attribute { kind: 3, data: 1 },
    };
    assert_eq!(
        downgrade(&mut down, &[pitched]),
        [Midi1Message::NoteOn {
            note: 60,
            velocity: 127
        }]
    );
    assert_eq!(down.lost(), 7);
}

#[test]
fn a_midi_2_packet_of_a_reserved_status_is_dropped_and_counted() {
    // Status 0x7, which no MIDI 2.0 channel voice message has, and which a
    // MIDI 1.0 destination may not be sent.
    let reserved = Event::new(0, [0x4070_3C00, 0xFFFF_0000, 0, 0]);
    let mut down = Downgrade::new();
    let mut out = EventList::with_capacity(4);
    down.block(&[reserved], &mut out);
    assert_eq!(out.as_slice(), []);
    assert_eq!(down.lost(), 1);
}

#[test]
fn the_types_a_midi_1_destination_takes_in_ump_pass_as_they_are() {
    // Utility, system, MIDI 1.0 channel voice, SysEx, SysEx 8, flex data and
    // stream packets: a UMP MIDI 1.0 device may use each.
    let others = [
        Event::new(0, [0x0020_0400, 0, 0, 0]),
        Event::new(0, [0x10F8_0000, 0, 0, 0]),
        Event::new(0, [0x20B0_0740, 0, 0, 0]),
        Event::new(0, [0x3016_7E7F, 0x0D70_0201, 0, 0]),
        Event::new(0, [0x5002_0001, 0x0200_0000, 0, 0]),
        Event::new(0, [0xD010_0000, 0x0098_9680, 0, 0]),
        Event::new(0, [0xF000_0101, 0x0000_001F, 0, 0]),
    ];
    let mut down = Downgrade::new();
    let mut out = EventList::with_capacity(8);
    down.block(&others, &mut out);
    assert_eq!(out.as_slice(), others);
    assert_eq!(down.lost(), 0);
}

/// Every MIDI 1.0 message, each data entry with its whole address and both
/// bytes, one sequence per message.
fn every_midi_1_message() -> Vec<Vec<Midi1Message>> {
    let cc = |controller, value| Midi1Message::ControlChange { controller, value };
    let mut all = Vec::new();
    for value in 0..=127 {
        for controller in (0..=127).filter(|c| !matches!(c, 0 | 6 | 32 | 38 | 98..=101)) {
            all.push(vec![cc(controller, value)]);
        }
        all.push(vec![Midi1Message::NoteOn {
            note: 60,
            velocity: value.max(1),
        }]);
        all.push(vec![Midi1Message::NoteOff {
            note: 60,
            velocity: value,
        }]);
        all.push(vec![Midi1Message::PolyPressure { note: 60, value }]);
        all.push(vec![Midi1Message::ChannelPressure { value }]);
        all.push(vec![
            cc(0, value),
            cc(32, 127 - value),
            Midi1Message::ProgramChange { program: value },
        ]);
    }
    for value in (0..=0x3FFF_u16).step_by(7) {
        let (high, low) = ((value >> 7) as u8, (value & 0x7F) as u8);
        all.push(vec![Midi1Message::PitchBend { value }]);
        all.push(vec![cc(101, 0), cc(100, 1), cc(6, high), cc(38, low)]);
        all.push(vec![cc(99, 3), cc(98, 4), cc(6, high), cc(38, low)]);
    }
    all
}

#[test]
fn midi_1_comes_back_from_midi_2_as_it_was_sent() {
    // D.1.2: MIDI 1.0 to MIDI 2.0 to MIDI 1.0 gives back the same data.
    let mut up = Upgrade::new();
    let mut down = Downgrade::new();
    for sent in every_midi_1_message() {
        let input: Vec<Event> = sent.iter().map(|&message| one(message).event(0)).collect();
        let mut upgraded = EventList::with_capacity(8);
        up.block(&input, LAST_FRAME, &mut upgraded);
        let mut back = EventList::with_capacity(8);
        down.block(upgraded.as_slice(), &mut back);
        let back: Vec<Midi1Message> = back
            .as_slice()
            .iter()
            .map(|event| event.midi1_voice().expect("MIDI 1.0").message)
            .collect();
        assert_eq!(back, sent);
    }
    assert_eq!(down.lost(), 0);
}

#[test]
fn a_note_on_at_velocity_0_comes_back_as_its_equivalent() {
    // A note-off, which MIDI 1.0 reads the same.
    let sent = [one(Midi1Message::NoteOn {
        note: 60,
        velocity: 0,
    })
    .event(0)];
    let mut upgraded = EventList::with_capacity(2);
    Upgrade::new().block(&sent, LAST_FRAME, &mut upgraded);
    let mut back = EventList::with_capacity(2);
    Downgrade::new().block(upgraded.as_slice(), &mut back);
    let message = back.as_slice()[0].midi1_voice().map(|voice| voice.message);
    assert_eq!(
        message,
        Some(Midi1Message::NoteOff {
            note: 60,
            velocity: 64
        })
    );
}
