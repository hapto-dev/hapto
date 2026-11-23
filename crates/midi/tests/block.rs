//! A block's MIDI: decoded once into a list, read whole or in parts, and
//! the MIDI a block sends, encoded back into packets.

use hapto_midi::{
    Centred, Channel, Decoder, Group, Key, Message, Midi, MidiList, MidiOut, NoteId, NoteKey,
    NoteOff, NoteOn, Pitch, Program,
};
use hapto_ump::{Event, EventList, Origin};
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

fn block(packets: &[(u32, [u32; 2])]) -> EventList {
    let mut list = EventList::with_capacity(16);
    for &(offset, [first, second]) in packets {
        list.push(Event::new(offset, [first, second, 0, 0]));
    }
    list
}

fn on(key: u32) -> [u32; 2] {
    [0x4090_0000 | key << 8, 0xFFFF_0000]
}

fn frames(midi: Midi<'_>) -> Vec<usize> {
    midi.iter().map(|(frame, _)| frame).collect()
}

#[test]
fn a_part_of_the_block_has_its_messages_at_frames_from_its_own_start() {
    let mut list = MidiList::new();
    list.read(&block(&[(2, on(60)), (40, on(62)), (63, on(64))]), 64);
    assert_eq!(frames(list.midi()), [2, 40, 63]);
    assert_eq!(frames(list.between(0, 32)), [2]);
    assert_eq!(frames(list.between(32, 64)), [8, 31]);
    let packets: Vec<u32> = list.between(32, 64).packets().map(|p| p.offset).collect();
    assert_eq!(packets, [8, 31]);
}

#[test]
fn a_packet_past_the_block_or_before_the_one_before_it_keeps_time_order() {
    let mut list = MidiList::new();
    list.read(&block(&[(10, on(60)), (4, on(62)), (99, on(64))]), 16);
    // The second at the first's frame, the third at the block's last.
    assert_eq!(frames(list.midi()), [10, 10, 15]);
}

#[test]
fn the_blocks_drops_come_with_the_part_that_starts_it() {
    let mut packets = block(&[(0, on(60))]);
    packets.add_dropped(2);
    let mut list = MidiList::with_room(1, 0, Decoder::new());
    // The block's own two drops, which the part from frame 0 reports.
    list.read(&packets, 8);
    assert_eq!(list.midi().dropped(), 2);
    assert_eq!(list.between(4, 8).dropped(), 0);
    // The key has two notes now, one from the block before. With room for
    // one message and one packet, the note-off's packet has no room, nor
    // have the two note-offs it decodes to.
    list.read(&block(&[(0, on(60)), (1, [0x4080_3C00, 0])]), 8);
    assert_eq!((list.midi().len(), list.midi().dropped()), (1, 3));
}

#[test]
fn every_sysex_of_a_block_reads_whole_and_one_past_the_room_is_dropped() {
    let complete =
        |first: u8, second: u8| [0x3002_0000 | u32::from(first) << 8 | u32::from(second), 0];
    let mut list = MidiList::with_room(8, 4, Decoder::new());
    list.read(
        &block(&[
            (0, complete(1, 2)),
            (1, complete(3, 4)),
            (2, complete(5, 6)),
        ]),
        8,
    );
    let data: Vec<Vec<u8>> = list
        .midi()
        .iter()
        .filter_map(|(_, message)| match message {
            Message::SysEx { data, .. } => Some(data.to_vec()),
            _ => None,
        })
        .collect();
    assert_eq!(data, [vec![1, 2], vec![3, 4]]);
    assert_eq!(list.midi().dropped(), 1, "the third had no room");
}

#[test]
fn a_note_spans_blocks_and_a_reset_forgets_it() {
    let mut list = MidiList::new();
    list.read(&block(&[(0, on(60))]), 8);
    let Some((_, Message::NoteOn(note))) = list.midi().iter().next() else {
        panic!("a note-on");
    };
    list.read(&block(&[(3, [0x4080_3C00, 0])]), 8);
    let Some((3, Message::NoteOff(off))) = list.midi().iter().next() else {
        panic!("a note-off");
    };
    assert_eq!(off.id, note.id, "the note began a block before");
    list.reset();
    assert!(list.midi().is_empty());
}

#[test]
fn a_block_with_no_midi_reads_as_none() {
    let midi = Midi::default();
    assert!(midi.is_empty() && midi.iter().next().is_none() && midi.packets().next().is_none());
    assert_eq!(midi.dropped(), 0);
}

#[test]
fn what_a_block_sends_goes_out_in_order_and_past_the_room_is_counted() {
    let mut sent = EventList::with_capacity(4);
    let mut out = MidiOut::new(&mut sent);
    out.send(1, NoteOn::new(Key::new(67), 0.5));
    out.send_packet(Event::new(2, [0x10F8_0000, 0, 0, 0]));
    // Eight bytes of SysEx: two packets.
    out.send(
        3,
        Message::SysEx {
            group: Group::FIRST,
            data: &[1, 2, 3, 4, 5, 6, 7, 8],
        },
    );
    out.send_packet(Event::new(4, [0x10F8_0000, 0, 0, 0]));
    assert_eq!(out.dropped(), 1);
    let heads: Vec<(u32, u32)> = sent
        .as_slice()
        .iter()
        .map(|e| (e.offset, e.words[0]))
        .collect();
    assert_eq!(
        heads,
        [
            (1, 0x4090_4300),
            (2, 0x10F8_0000),
            (3, 0x3016_0102),
            (3, 0x3032_0708)
        ]
    );
}

#[test]
fn a_note_its_host_named_goes_out_under_the_hosts_id_and_one_made_up_under_none() {
    // A host's note 7 on C4, a pressure on it, and its note-off, as a block
    // decodes them, then a note the decoder makes up.
    let named = |words: [u32; 2]| Event {
        origin: Origin::Note { id: 7 },
        ..Event::new(0, [words[0], words[1], 0, 0])
    };
    let mut list = EventList::with_capacity(8);
    for words in [on(60), [0x40A0_3C00, u32::MAX], [0x4080_3C00, 0]] {
        list.push(named(words));
    }
    list.push(Event::new(1, [0x4090_4000, 0xFFFF_0000, 0, 0]));
    let mut midi = MidiList::new();
    midi.read(&list, 16);
    let mut sent = EventList::with_capacity(8);
    let mut out = MidiOut::new(&mut sent);
    for (frame, message) in midi.midi() {
        out.forward(frame, &message);
    }
    let origins: Vec<Origin> = sent.as_slice().iter().map(|e| e.origin).collect();
    let host = Origin::Note { id: 7 };
    assert_eq!(origins, [host, host, host, Origin::Midi2]);
}

fn unit() -> impl Strategy<Value = f32> {
    (0_u32..=1000).prop_map(|n| n as f32 / 1000.0)
}

fn centred() -> impl Strategy<Value = Centred> {
    (-1000_i32..=1000).prop_map(|n| Centred::new(n as f32 / 1000.0))
}

fn place() -> impl Strategy<Value = (Group, Channel)> {
    (1_u8..=16, 1_u8..=16).prop_map(|(g, c)| (Group::number(g), Channel::number(c)))
}

/// A note's own key, on no note the decoder holds.
fn note_key() -> impl Strategy<Value = NoteKey> {
    (place(), 0_u8..128).prop_map(|((group, channel), key)| NoteKey {
        group,
        channel,
        key: Key::new(key),
        id: None,
    })
}

/// Every message the decoder makes but SysEx, which comes back as sent:
/// a note under an id the decoder makes up, which is never the sender's.
fn any_message() -> impl Strategy<Value = Message<'static>> {
    prop_oneof![note_message(), channel_message()]
}

/// A note, or a note's own message.
fn note_message() -> impl Strategy<Value = Message<'static>> {
    prop_oneof![
        (place(), 0_u8..128, any::<u16>()).prop_map(|((group, channel), key, velocity)| {
            let mut on = NoteOn::new(Key::new(key), f32::from(velocity) / f32::from(u16::MAX));
            on.group = group;
            on.channel = channel;
            Message::NoteOn(on)
        }),
        (place(), 0_u8..128, any::<u16>()).prop_map(|((group, channel), key, velocity)| {
            Message::NoteOff(NoteOff {
                id: NoteId::own(0),
                group,
                channel,
                key: Key::new(key),
                velocity: f32::from(velocity) / f32::from(u16::MAX),
            })
        }),
        // A note's registered controllers but its pitch, which comes back as
        // NotePitch.
        (
            note_key(),
            (0_u8..=255).prop_filter("not the pitch", |i| *i != 3),
            unit()
        )
            .prop_map(|(note, controller, value)| Message::NoteControl {
                note,
                controller,
                value
            }),
        (note_key(), any::<u8>(), unit())
            .prop_map(|(note, index, value)| { Message::NoteAssignable { note, index, value } }),
        (note_key(), centred()).prop_map(|(note, value)| Message::NoteBend { note, value }),
        (note_key(), any::<bool>(), any::<bool>()).prop_map(|(note, detach, reset)| {
            Message::NoteManagement {
                note,
                detach,
                reset,
            }
        }),
    ]
}

/// A channel's message, or a note's pressure or pitch.
fn channel_message() -> impl Strategy<Value = Message<'static>> {
    prop_oneof![
        (place(), centred()).prop_map(|((group, channel), value)| Message::Bend {
            group,
            channel,
            value
        }),
        (place(), unit()).prop_map(|((group, channel), value)| Message::Pressure {
            group,
            channel,
            value
        }),
        (place(), 0_u8..128, unit()).prop_map(|((group, channel), controller, value)| {
            Message::Control {
                group,
                channel,
                controller,
                value,
            }
        }),
        (place(), 0_u8..128, 1_u8..128, unit()).prop_map(
            |((group, channel), bank, index, value)| {
                Message::Registered {
                    group,
                    channel,
                    bank,
                    index,
                    value,
                }
            }
        ),
        (place(), 0_u8..128, 0_u8..128, unit()).prop_map(
            |((group, channel), bank, index, value)| {
                Message::Assignable {
                    group,
                    channel,
                    bank,
                    index,
                    value,
                }
            }
        ),
        (place(), 1_u8..=128, prop::option::of(0_u16..16384)).prop_map(
            |((group, channel), number, bank)| Message::Program {
                group,
                channel,
                program: Program::number(number),
                bank,
            }
        ),
        (place(), 0_u8..100, 0_u8..100).prop_map(|((group, channel), semitones, cents)| {
            Message::BendRange {
                group,
                channel,
                semitones: f32::from(semitones) + f32::from(cents) / 100.0,
            }
        }),
        (place(), 0_u8..128, unit()).prop_map(|((group, channel), key, value)| {
            Message::NotePressure {
                note: NoteKey {
                    group,
                    channel,
                    key: Key::new(key),
                    id: None,
                },
                value,
            }
        }),
        (place(), 0_u8..128, 0_u16..(127 * 512)).prop_map(|((group, channel), key, pitch)| {
            Message::NotePitch {
                note: NoteKey {
                    group,
                    channel,
                    key: Key::new(key),
                    id: None,
                },
                pitch: Pitch::new(f32::from(pitch) / 512.0),
            }
        }),
    ]
}

/// Whether two messages say the same, values within what 32 bits and an
/// `f32` keep.
fn alike(sent: &Message<'_>, back: &Message<'_>) -> bool {
    let near = |a: f32, b: f32| (a - b).abs() <= 1e-6;
    match (sent, back) {
        // A note's id is the decoder's to make up.
        (Message::NoteOn(a), Message::NoteOn(b)) => {
            (a.group, a.channel, a.key) == (b.group, b.channel, b.key)
                && near(a.velocity, b.velocity)
        }
        (Message::NoteOff(a), Message::NoteOff(b)) => {
            (a.group, a.channel, a.key) == (b.group, b.channel, b.key)
                && near(a.velocity, b.velocity)
        }
        (Message::NoteBend { value: a, .. }, Message::NoteBend { value: b, .. }) => {
            near(a.get(), b.get())
        }
        (Message::NoteControl { value: a, .. }, Message::NoteControl { value: b, .. })
        | (Message::NoteAssignable { value: a, .. }, Message::NoteAssignable { value: b, .. }) => {
            near(*a, *b)
        }
        (Message::Bend { value: a, .. }, Message::Bend { value: b, .. }) => near(a.get(), b.get()),
        (Message::Pressure { value: a, .. }, Message::Pressure { value: b, .. })
        | (Message::Control { value: a, .. }, Message::Control { value: b, .. })
        | (Message::Registered { value: a, .. }, Message::Registered { value: b, .. })
        | (Message::Assignable { value: a, .. }, Message::Assignable { value: b, .. })
        | (Message::NotePressure { value: a, .. }, Message::NotePressure { value: b, .. }) => {
            near(*a, *b)
        }
        (Message::BendRange { semitones: a, .. }, Message::BendRange { semitones: b, .. }) => {
            near(*a, *b)
        }
        (Message::NotePitch { pitch: a, .. }, Message::NotePitch { pitch: b, .. }) => {
            near(a.semitones(), b.semitones())
        }
        _ => sent == back,
    }
}

/// The same message with its values put aside, to compare the rest.
fn shape(message: &Message<'_>) -> String {
    let text = format!("{message:?}");
    text.split("value")
        .next()
        .unwrap_or("")
        .split("pitch")
        .next()
        .unwrap_or("")
        .to_owned()
}

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "tests/block.proptest-regressions",
        ))),
        cases: 1024,
        ..ProptestConfig::default()
    })]

    /// A message sent and read back says what it said, its values within
    /// what 32 bits and an `f32` keep.
    #[test]
    fn a_message_sent_reads_back_as_itself(message in any_message()) {
        let mut sent = EventList::with_capacity(4);
        MidiOut::new(&mut sent).send(0, message);
        let mut list = MidiList::new();
        list.read(&sent, 1);
        let back: Vec<Message<'_>> = list.midi().iter().map(|(_, m)| m).collect();
        prop_assert_eq!(back.len(), 1);
        prop_assert!(alike(&message, &back[0]), "{:?} came back as {:?}", message, back[0]);
        if !matches!(message, Message::NoteOn(_) | Message::NoteOff(_)) {
            prop_assert_eq!(shape(&message), shape(&back[0]));
        }
    }

    /// A SysEx of any length sent and read back is itself.
    #[test]
    fn a_sysex_sent_reads_back_whole(data in prop::collection::vec(0_u8..0x80, 0..40)) {
        let mut sent = EventList::with_capacity(16);
        MidiOut::new(&mut sent).send(0, Message::SysEx { group: Group::number(3), data: &data });
        let mut list = MidiList::new();
        list.read(&sent, 1);
        let back: Vec<Message<'_>> = list.midi().iter().map(|(_, m)| m).collect();
        prop_assert_eq!(back, [Message::SysEx { group: Group::number(3), data: &data }]);
    }
}

/// `data` sent as SysEx on `group`, its packets.
fn sysex_packets(group: Group, data: &[u8]) -> Vec<Event> {
    let mut sent = EventList::with_capacity(16);
    MidiOut::new(&mut sent).send(0, Message::SysEx { group, data });
    sent.as_slice().to_vec()
}

/// Reads `packets` as one block into `list`, and returns what it decoded.
fn read_block(list: &mut MidiList, packets: &[Event]) -> Vec<String> {
    let mut block = EventList::with_capacity(16);
    for &packet in packets {
        block.push(packet);
    }
    list.read(&block, 1);
    list.midi().iter().map(|(_, m)| format!("{m:?}")).collect()
}

#[test]
fn a_sysex_split_across_two_blocks_comes_whole_in_the_second() {
    let data: Vec<u8> = (0..20).collect();
    let packets = sysex_packets(Group::FIRST, &data);
    assert_eq!(packets.len(), 4);
    let mut list = MidiList::new();
    assert!(
        read_block(&mut list, &packets[..2]).is_empty(),
        "not whole yet"
    );
    let whole = read_block(&mut list, &packets[2..]);
    let expected = Message::SysEx {
        group: Group::FIRST,
        data: &data,
    };
    assert_eq!(whole, [format!("{expected:?}")]);
}

#[test]
fn a_new_sysex_on_its_group_cuts_the_one_under_way_short() {
    let (first, second) = ([1_u8; 12], [2_u8; 3]);
    let mut packets = sysex_packets(Group::FIRST, &first);
    // The first's start alone, then the second whole.
    packets.truncate(1);
    packets.extend(sysex_packets(Group::FIRST, &second));
    let mut list = MidiList::new();
    let decoded = read_block(&mut list, &packets);
    let expected = Message::SysEx {
        group: Group::FIRST,
        data: &second,
    };
    assert_eq!(decoded, [format!("{expected:?}")]);
    assert_eq!(list.midi().dropped(), 1, "the first, cut short");
}
