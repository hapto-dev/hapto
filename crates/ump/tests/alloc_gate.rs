//! The UMP crate's audio-thread calls neither allocate nor free: a block's
//! list filled past its capacity, read, changed in place and cleared, words
//! split into packets, channel voice messages decoded and encoded, a block
//! upgraded from MIDI 1.0 and downgraded back, and a device's byte stream
//! read into packets and written back.

hapto_testkit::install_alloc_gate!();

use hapto_testkit::alloc::assert_no_allocations;
use hapto_ump::{
    Attribute, Bank, ByteStream, Bytes, ChannelMessage, ChannelVoice, Downgrade, Event, EventList,
    Midi1Message, Midi1Voice, Packets, Upgrade,
};

const CAPACITY: usize = 256;

/// The group and channel pairs every message goes out on.
const PLACES: [(u8, u8); 4] = [(0, 0), (0, 9), (15, 0), (15, 9)];

/// Every MIDI 2.0 channel voice message, on two groups and two channels.
fn voices() -> Vec<ChannelVoice> {
    let attribute = Attribute { kind: 3, data: 1 };
    let messages = [
        ChannelMessage::NoteOff {
            note: 60,
            velocity: 0,
            attribute: Attribute::NONE,
        },
        ChannelMessage::NoteOn {
            note: 60,
            velocity: u16::MAX,
            attribute,
        },
        ChannelMessage::PolyPressure { note: 60, value: 1 },
        ChannelMessage::ControlChange {
            controller: 7,
            value: u32::MAX,
        },
        ChannelMessage::ProgramChange {
            program: 5,
            bank: Some(Bank { msb: 1, lsb: 2 }),
        },
        ChannelMessage::ChannelPressure { value: 1 },
        ChannelMessage::PitchBend { value: 1 << 31 },
        ChannelMessage::RegisteredController {
            bank: 0,
            index: 1,
            value: 1,
        },
        ChannelMessage::AssignableController {
            bank: 2,
            index: 44,
            value: 1 << 31,
        },
        ChannelMessage::RelativeRegisteredController {
            bank: 0,
            index: 1,
            change: -1,
        },
        ChannelMessage::RelativeAssignableController {
            bank: 0,
            index: 1,
            change: 1,
        },
        ChannelMessage::RegisteredPerNoteController {
            note: 60,
            index: 255,
            value: 1,
        },
        ChannelMessage::AssignablePerNoteController {
            note: 60,
            index: 1,
            value: 1,
        },
        ChannelMessage::PerNotePitchBend { note: 60, value: 1 },
        ChannelMessage::PerNoteManagement {
            note: 60,
            detach: true,
            reset: false,
        },
    ];
    PLACES
        .iter()
        .flat_map(|&(group, channel)| {
            messages.map(|message| ChannelVoice {
                group,
                channel,
                message,
            })
        })
        .collect()
}

/// Every MIDI 1.0 channel voice message, data entries with their address,
/// on two groups and two channels.
fn midi1_voices() -> Vec<Midi1Voice> {
    let cc = |controller, value| Midi1Message::ControlChange { controller, value };
    let messages = [
        Midi1Message::NoteOff {
            note: 60,
            velocity: 0,
        },
        Midi1Message::NoteOn {
            note: 60,
            velocity: 127,
        },
        Midi1Message::NoteOn {
            note: 60,
            velocity: 0,
        },
        Midi1Message::PolyPressure { note: 60, value: 1 },
        cc(7, 100),
        cc(0, 1),
        cc(32, 2),
        Midi1Message::ProgramChange { program: 5 },
        Midi1Message::ChannelPressure { value: 1 },
        Midi1Message::PitchBend { value: 0x2000 },
        cc(99, 2),
        cc(98, 44),
        cc(6, 64),
        cc(38, 1),
        // A high byte left held until the end of the block.
        cc(101, 0),
        cc(100, 0),
        cc(6, 12),
    ];
    PLACES
        .iter()
        .flat_map(|&(group, channel)| {
            messages.map(|message| Midi1Voice {
                group,
                channel,
                message,
            })
        })
        .collect()
}

#[test]
fn filling_reading_and_clearing_a_list_does_not_allocate() {
    // Past the capacity, so the full list's drop count runs too.
    let events: Vec<Event> = voices()
        .iter()
        .cycle()
        .take(CAPACITY + 8)
        .zip(0..)
        .map(|(voice, offset)| voice.event(offset))
        .collect();
    let mut list = EventList::with_capacity(CAPACITY);

    let (len, dropped) = assert_no_allocations("an event list filled, read and cleared", || {
        let mut seen = (0, 0);
        for _ in 0..4 {
            list.clear();
            for &event in &events {
                list.push(event);
            }
            for event in list.as_mut_slice() {
                event.offset += 64;
            }
            let mut sum = 0_u64;
            for event in list.as_slice() {
                sum += u64::from(event.message_type().get()) + u64::from(event.group());
                if let Some(voice) = event.channel_voice() {
                    sum += u64::from(voice.event(event.offset).words[0]);
                }
            }
            list.add_dropped(1);
            std::hint::black_box(sum);
            seen = (list.as_slice().len(), list.dropped());
        }
        seen
    });
    assert_eq!((len, dropped), (CAPACITY, 9), "filled to capacity, past it");
}

#[test]
fn splitting_words_and_decoding_and_encoding_voices_does_not_allocate() {
    // Every voice as its packet's words, packets of the other sizes Hapto
    // skips, and a MIDI 2.0 note-on cut after its first word at the end.
    let packets = voices()
        .iter()
        .map(ChannelVoice::words)
        .chain(midi1_voices().iter().map(Midi1Voice::words))
        .collect::<Vec<_>>();
    let mut words = Vec::new();
    for packet in &packets {
        let length = Event::new(0, *packet).message_type().words();
        words.extend_from_slice(&packet[..length]);
    }
    words.extend([0x0000_0000, 0xB000_0000, 0, 0, 0x5000_0000, 0, 0, 0]);
    words.push(0x4090_3C00);

    let (voices, cut) = assert_no_allocations("packets split, decoded and encoded", || {
        let (mut voices, mut cut) = (0, 0);
        for packet in Packets::new(&words, 32) {
            match packet {
                Ok(event) => {
                    if let Some(voice) = event.channel_voice() {
                        assert_eq!(voice.words(), event.words, "encodes back");
                        voices += 1;
                    }
                    if let Some(voice) = event.midi1_voice() {
                        assert_eq!(voice.words(), event.words, "encodes back");
                        voices += 1;
                    }
                }
                Err(truncated) => cut += truncated.needed - truncated.available,
            }
        }
        (voices, cut)
    });
    assert_eq!((voices, cut), (packets.len(), 1));
}

#[test]
fn upgrading_and_downgrading_a_block_does_not_allocate() {
    let at_frames = |events: &mut dyn Iterator<Item = Event>| -> Vec<Event> {
        events
            .zip(0..)
            .map(|(event, offset)| Event { offset, ..event })
            .collect()
    };
    let sent = at_frames(&mut midi1_voices().iter().map(|voice| voice.event(0)));
    let midi2 = at_frames(&mut voices().iter().map(|voice| voice.event(0)));
    // Making an upgrade allocates its channels' state; using it does not.
    let mut upgrade = Upgrade::new();
    let mut downgrade = Downgrade::new();
    let mut upgraded = EventList::with_capacity(CAPACITY);
    let mut back = EventList::with_capacity(CAPACITY);

    let seen = assert_no_allocations("a block upgraded and downgraded", || {
        let mut seen = (0, 0, 0);
        for _ in 0..4 {
            upgraded.clear();
            back.clear();
            upgrade.block(&sent, 255, &mut upgraded);
            downgrade.block(upgraded.as_slice(), &mut back);
            let up = upgraded.as_slice().len();
            let down = back.as_slice().len();
            back.clear();
            downgrade.block(&midi2, &mut back);
            seen = (up, down, back.as_slice().len());
        }
        seen
    });
    // Each place's 17 messages upgrade to 10: bank select and the
    // addresses send nothing of their own, and each data entry is one
    // controller. They downgrade to 18: the bank goes out with its
    // program, and the held high byte with its low byte.
    assert_eq!(seen.0, PLACES.len() * 10);
    assert_eq!(seen.1, PLACES.len() * 18);
    // Of MIDI 2.0's 15 messages, MIDI 1.0 loses the relative and per-note
    // six, and the other nine are 17: the bank and the two controllers
    // grow. The note-on's attribute is lost too, on each of four rounds.
    assert_eq!(seen.2, PLACES.len() * 17);
    assert_eq!(downgrade.lost(), 4 * 7 * PLACES.len() as u64);
}

#[test]
fn reading_a_byte_stream_and_writing_it_back_does_not_allocate() {
    // Running status, a clock inside a note, a system common message, and
    // a SysEx long enough to take three packets, cut short at the end.
    let mut stream: Vec<u8> = vec![0x90, 0x3C, 0xF8, 0x7F, 0x3E, 0x7F, 0xF2, 0x10, 0x20, 0xF0];
    stream.extend(1..=15);
    stream.extend([0xF7, 0xF0, 0x01]);
    let mut bytes = ByteStream::new(3);

    let (packets, written) = assert_no_allocations("a byte stream read and written", || {
        let mut seen = (0, 0);
        for _ in 0..4 {
            bytes.reset();
            let (mut packets, mut written) = (0, 0);
            for chunk in stream.chunks(4) {
                bytes.read(chunk, |words| {
                    packets += 1;
                    written += Bytes::of(words).map_or(0, |bytes| bytes.len());
                });
            }
            seen = (packets, written);
        }
        seen
    });
    // Two notes, the clock, song position and the SysEx's three packets;
    // the SysEx cut short is held. Each writes in full: the second note's
    // status, which running status left out, comes back.
    assert_eq!(packets, 7);
    assert_eq!(written, 3 + 1 + 3 + 3 + 1 + 15 + 1);
}
