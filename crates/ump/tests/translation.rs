//! Tests the Default Translation between MIDI 1.0 and MIDI 2.0 in appendix D
//! of the UMP and MIDI 2.0 Protocol specification. It covers an exporter's
//! upgrade on the way in, the downgrade on the way out, and the round trip,
//! which must give back what MIDI 1.0 sent.

use hapto_ump::{
    Attribute, Bank, ChannelMessage, ChannelVoice, Downgrade, Event, EventList, Midi1Message,
    Midi1Voice, Origin, Upgrade, scale,
};

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

/// Upgrades `sent` as one block and returns the output with its frames.
fn upgrade(upgrade: &mut Upgrade, sent: &[(u32, Midi1Voice)]) -> Vec<(u32, ChannelMessage)> {
    let input: Vec<Event> = sent
        .iter()
        .map(|(frame, voice)| voice.event(*frame))
        .collect();
    let mut out = EventList::with_capacity(64);
    upgrade.block(&input, &mut out);
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
            // A velocity of 1 becomes 0x0200 (appendix D.3.1).
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
fn an_nrpn_s_low_byte_at_a_later_frame_refines_its_high_byte() {
    let sent = [
        (0, cc(99, 2)),
        (0, cc(98, 5)),
        (4, cc(6, 100)),
        (9, cc(38, 3)),
    ];
    assert_eq!(
        upgrade(&mut Upgrade::new(), &sent),
        [
            (4, assignable(2, 5, 100 << 7)),
            (9, assignable(2, 5, 100 << 7 | 3)),
        ]
    );
}

#[test]
fn a_high_byte_alone_goes_out_at_its_own_frame() {
    let mut up = Upgrade::new();
    let sent = [(0, cc(99, 2)), (0, cc(98, 5)), (4, cc(6, 100))];
    assert_eq!(upgrade(&mut up, &sent), [(4, assignable(2, 5, 100 << 7))]);
    // A low byte in the next block refines it.
    assert_eq!(
        upgrade(&mut up, &[(2, cc(38, 3))]),
        [(2, assignable(2, 5, 100 << 7 | 3))]
    );
}

#[test]
fn a_high_byte_sent_alone_is_marked_apart_from_a_low_byte_of_0() {
    // Both have the same value, but a reader at the sent width must tell a
    // missing low byte apart from a low byte of 0.
    let mut up = Upgrade::new();
    let mut out = EventList::with_capacity(4);
    let high = [cc(99, 2).event(0), cc(98, 5).event(0), cc(6, 64).event(1)];
    up.block(&high, &mut out);
    up.block(&[cc(38, 0).event(0)], &mut out);
    let [alone, refined] = out.as_slice() else {
        panic!("two messages: {:?}", out.as_slice());
    };
    assert_eq!(
        [
            (alone.offset, alone.origin),
            (refined.offset, refined.origin)
        ],
        [(1, Origin::Midi1HighByte), (0, Origin::Midi1)]
    );
    assert_eq!(alone.words, refined.words);
}

#[test]
fn a_low_byte_joins_its_high_byte_only_as_the_packet_just_sent() {
    // Another channel's note between the two at their frame: the high byte
    // has gone out before the note, so its low byte refines it after.
    let other = Midi1Voice {
        channel: 1,
        ..one(Midi1Message::NoteOn {
            note: 60,
            velocity: 100,
        })
    };
    let sent = [cc(99, 2), cc(98, 5), cc(6, 10), other, cc(38, 3)];
    let input: Vec<Event> = sent.iter().map(|voice| voice.event(3)).collect();
    let mut out = EventList::with_capacity(8);
    Upgrade::new().block(&input, &mut out);
    let origins: Vec<Origin> = out.as_slice().iter().map(|event| event.origin).collect();
    assert_eq!(
        origins,
        [Origin::Midi1HighByte, Origin::Midi1, Origin::Midi1]
    );
    let last = out.as_slice().last().and_then(Event::channel_voice);
    assert_eq!(
        last.map(|voice| voice.message),
        Some(assignable(2, 5, 10 << 7 | 3))
    );
}

/// Upgrades `sent`, each a MIDI 1.0 message at a frame of the stream, in
/// blocks of `frames`, and returns every packet at its frame in the stream.
fn upgrade_in_blocks(sent: &[(u32, Midi1Voice)], frames: u32) -> Vec<Event> {
    let mut up = Upgrade::new();
    let end = sent.iter().map(|(frame, _)| frame + 1).max().unwrap_or(0);
    let mut upgraded = Vec::new();
    for start in (0..end).step_by(frames as usize) {
        let input: Vec<Event> = sent
            .iter()
            .filter(|(frame, _)| (start..start + frames).contains(frame))
            .map(|(frame, voice)| voice.event(frame - start))
            .collect();
        let mut out = EventList::with_capacity(64);
        up.block(&input, &mut out);
        upgraded.extend(out.as_slice().iter().map(|event| Event {
            offset: start + event.offset,
            ..*event
        }));
    }
    upgraded
}

#[test]
fn a_high_byte_alone_goes_out_before_the_channel_s_later_messages() {
    // A bend range of 12 set by its high byte alone, then a bend and a note,
    // in a block of 512: the note bends by the new range.
    let note = one(Midi1Message::NoteOn {
        note: 60,
        velocity: 100,
    });
    let sent = [
        (0, cc(101, 0)),
        (0, cc(100, 0)),
        (1, cc(6, 12)),
        (2, one(Midi1Message::PitchBend { value: 0x3FFF })),
        (3, note),
    ];
    let upgraded = upgrade_in_blocks(&sent, 512);
    let range = ChannelMessage::RegisteredController {
        bank: 0,
        index: 0,
        value: scale::up_14_to_32(12 << 7),
    };
    let first = upgraded.first().map(|event| (event.offset, event.origin));
    assert_eq!(first, Some((1, Origin::Midi1HighByte)));
    let messages: Vec<(u32, ChannelMessage)> = upgraded
        .iter()
        .map(|event| {
            (
                event.offset,
                event.channel_voice().expect("MIDI 2.0").message,
            )
        })
        .collect();
    assert_eq!(
        messages,
        [
            (1, range),
            (2, ChannelMessage::PitchBend { value: u32::MAX }),
            (
                3,
                ChannelMessage::NoteOn {
                    note: 60,
                    velocity: scale::up_7_to_16(100),
                    attribute: Attribute::NONE
                }
            ),
        ]
    );
}

#[test]
fn an_mpe_zone_s_configuration_goes_out_before_its_member_s_note() {
    // The configuration is RPN 0/6 on the master channel with its high byte
    // alone; the note it places comes on a member channel at the same frame.
    let member = Midi1Voice {
        channel: 1,
        ..one(Midi1Message::NoteOn {
            note: 60,
            velocity: 100,
        })
    };
    let sent = [
        (0, cc(101, 0)),
        (0, cc(100, 6)),
        (0, cc(6, 15)),
        (0, member),
    ];
    let order: Vec<(u32, u8, ChannelMessage)> = upgrade_in_blocks(&sent, 512)
        .iter()
        .map(|event| {
            let voice = event.channel_voice().expect("MIDI 2.0");
            (event.offset, voice.channel, voice.message)
        })
        .collect();
    let configuration = ChannelMessage::RegisteredController {
        bank: 0,
        index: 6,
        value: scale::up_14_to_32(15 << 7),
    };
    let note = ChannelMessage::NoteOn {
        note: 60,
        velocity: scale::up_7_to_16(100),
        attribute: Attribute::NONE,
    };
    assert_eq!(order, [(0, 0, configuration), (0, 1, note)]);
}

#[test]
fn a_pair_a_block_cuts_goes_out_as_one_block_sends_it() {
    let sent = [
        (0, cc(99, 2)),
        (0, cc(98, 5)),
        (3, cc(6, 10)),
        (5, cc(38, 3)),
    ];
    let whole = upgrade_in_blocks(&sent, 8);
    assert_eq!(upgrade_in_blocks(&sent, 4), whole);
    // The high byte at its own frame, then its low byte refining it.
    let sent_at: Vec<(u32, Origin)> = whole
        .iter()
        .map(|event| (event.offset, event.origin))
        .collect();
    assert_eq!(sent_at, [(3, Origin::Midi1HighByte), (5, Origin::Midi1)]);
}

#[test]
fn each_high_byte_goes_out_at_its_frame_under_the_address_it_was_sent_under() {
    let sent = [
        (0, cc(99, 2)),
        (0, cc(98, 5)),
        (1, cc(6, 10)),
        (2, cc(6, 20)),
        (3, cc(99, 7)),
        (3, cc(98, 1)),
        (4, cc(6, 30)),
    ];
    assert_eq!(
        upgrade(&mut Upgrade::new(), &sent),
        [
            (1, assignable(2, 5, 10 << 7)),
            (2, assignable(2, 5, 20 << 7)),
            (4, assignable(7, 1, 30 << 7)),
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
    // Pitch bend sensitivity of 12 semitones.
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
fn a_lone_address_high_byte_keeps_the_low_byte_held() {
    // D.3.3 holds the latest of 98 to 101, so a sender that sends only the
    // address byte that changed reaches the number a MIDI 1.0 receiver
    // reaches: RPN 0/0 again, here.
    let registered = |value| ChannelMessage::RegisteredController {
        bank: 0,
        index: 0,
        value: scale::up_14_to_32(value),
    };
    let mut up = Upgrade::new();
    let first = [cc(101, 0), cc(100, 0), cc(6, 2), cc(38, 0)].map(|cc| (0, cc));
    assert_eq!(upgrade(&mut up, &first), [(0, registered(2 << 7))]);
    let again = [(1, cc(101, 0)), (2, cc(6, 3)), (3, cc(38, 0))];
    assert_eq!(
        upgrade(&mut up, &again),
        [(2, registered(3 << 7)), (3, registered(3 << 7))]
    );
    // NRPN 1/5, then 99 alone moves it to 2/5.
    let mut up = Upgrade::new();
    let first = [cc(99, 1), cc(98, 5), cc(6, 9), cc(38, 9)].map(|cc| (0, cc));
    assert_eq!(
        upgrade(&mut up, &first),
        [(0, assignable(1, 5, 9 << 7 | 9))]
    );
    let moved = [(1, cc(99, 2)), (2, cc(6, 3)), (3, cc(38, 0))];
    assert_eq!(
        upgrade(&mut up, &moved),
        [(2, assignable(2, 5, 3 << 7)), (3, assignable(2, 5, 3 << 7))]
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
fn a_low_byte_sent_first_is_forgotten_at_the_next_address_byte() {
    // NRPN 2/5's low byte, whose high byte came before the upgrade began;
    // another number's controller; a long wait; then 2/5's high byte. A
    // MIDI 1.0 receiver of the sender holds (20, 0).
    let mut up = Upgrade::new();
    let early = [(0, cc(99, 2)), (0, cc(98, 5)), (0, cc(38, 3))];
    assert_eq!(upgrade(&mut up, &early), []);
    let other = [(0, cc(99, 7)), (0, cc(98, 1)), (1, cc(7, 100))];
    assert_eq!(upgrade(&mut up, &other).len(), 1);
    for _ in 0..1000 {
        assert_eq!(upgrade(&mut up, &[]), []);
    }
    let high = [(0, cc(99, 2)), (0, cc(98, 5)), (4, cc(6, 20))];
    assert_eq!(upgrade(&mut up, &high), [(4, assignable(2, 5, 20 << 7))]);
}

#[test]
fn an_address_sent_again_keeps_its_high_byte_for_the_low_byte() {
    // The upgrade holds the latest data entry high byte (6), and a low byte
    // (38) goes out with it (appendix D.3.3).
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
            (1, assignable(2, 5, 100 << 7)),
            (3, assignable(2, 5, 100 << 7 | 3)),
        ]
    );
    // A change of the low byte alone, a block later.
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
fn reset_all_controllers_forgets_the_address() {
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
            (1, assignable(2, 5, 10 << 7)),
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
    // A high byte goes out at its own frame, before the step, so a MIDI 1.0
    // receiver steps the value it sets rather than the one before.
    let sent = [
        (0, cc(99, 2)),
        (0, cc(98, 5)),
        (1, cc(6, 100)),
        (1, cc(96, 1)),
    ];
    assert_eq!(
        upgrade(&mut Upgrade::new(), &sent),
        [
            (1, assignable(2, 5, 100 << 7)),
            (
                1,
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
    // Bank select (0 and 32) alone translates to nothing.
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
    // A program change carries the current bank (appendix D.3.4).
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
        [(3, assignable(2, 5, 10 << 7))]
    );
}

#[test]
fn a_low_byte_refines_its_own_channel_s_high_byte() {
    let on = |group, channel, voice: Midi1Voice| Midi1Voice {
        group,
        channel,
        ..voice
    };
    let mut sent = Vec::new();
    for (frame, (group, channel, high)) in (0..).zip([(3, 9, 30), (0, 15, 20), (0, 4, 10)]) {
        sent.extend([cc(99, 2), cc(98, 5), cc(6, high)].map(|cc| (frame, on(group, channel, cc))));
    }
    // Each high byte at its frame, then the low byte of one of them.
    sent.push((5, on(0, 15, cc(38, 1))));
    assert_eq!(
        upgrade(&mut Upgrade::new(), &sent),
        [
            (0, assignable(2, 5, 30 << 7)),
            (1, assignable(2, 5, 20 << 7)),
            (2, assignable(2, 5, 10 << 7)),
            (5, assignable(2, 5, 20 << 7 | 1)),
        ]
    );
}

#[test]
fn other_packets_pass_as_they_are() {
    let midi2 = Event::new(0, [0x40B0_0700, 1, 0, 0]);
    let sysex = Event::new(1, [0x3016_7E7F, 0x0D70_0201, 0, 0]);
    let mut out = EventList::with_capacity(4);
    Upgrade::new().block(&[midi2, sysex], &mut out);
    assert_eq!(out.as_slice(), [midi2, sysex]);
}

/// Downgrades `messages`, each at frame 0, and returns the output.
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
    // A note's attribute is lost, but the note still goes out.
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
    // No MIDI 2.0 channel voice message has status 0x7, and a MIDI 1.0
    // destination must not be sent one.
    let reserved = Event::new(0, [0x4070_3C00, 0xFFFF_0000, 0, 0]);
    let mut down = Downgrade::new();
    let mut out = EventList::with_capacity(4);
    down.block(&[reserved], &mut out);
    assert_eq!(out.as_slice(), []);
    assert_eq!(down.lost(), 1);
}

#[test]
fn the_types_a_midi_1_destination_takes_in_ump_pass_as_they_are() {
    // A UMP MIDI 1.0 device may use utility, system, MIDI 1.0 channel
    // voice, SysEx, SysEx 8, flex data and stream packets.
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

/// Returns every MIDI 1.0 message as one sequence each, with each data entry
/// given its whole address and both bytes.
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
    // MIDI 1.0 comes back unchanged from MIDI 2.0 (appendix D.1.2).
    let mut up = Upgrade::new();
    let mut down = Downgrade::new();
    for sent in every_midi_1_message() {
        let input: Vec<Event> = sent.iter().map(|&message| one(message).event(0)).collect();
        let mut upgraded = EventList::with_capacity(8);
        up.block(&input, &mut upgraded);
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
    // As a note-off, which MIDI 1.0 reads the same way.
    let sent = [one(Midi1Message::NoteOn {
        note: 60,
        velocity: 0,
    })
    .event(0)];
    let mut upgraded = EventList::with_capacity(2);
    Upgrade::new().block(&sent, &mut upgraded);
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
