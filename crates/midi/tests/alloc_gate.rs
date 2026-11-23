//! Decoding neither allocates nor frees: every kind of message, notes and
//! their ids, a note's own messages, and a SysEx joined across packets; nor
//! does reading a block's list whole and in parts, or sending.

hapto_testkit::install_alloc_gate!();

use hapto_midi::{Decoder, Key, Message, MidiList, MidiOut, NoteOn};
use hapto_testkit::alloc::assert_no_allocations;
use hapto_ump::{Event, EventList};

#[test]
fn decoding_every_kind_of_packet_does_not_allocate() {
    let packets: Vec<Event> = [
        [0x4090_3C00, 0xFFFF_0000],
        [0x4090_3C03, 0xFFFF_8A00],
        [0x40A0_3C00, 0x8000_0000],
        [0x4000_3C4A, 0x8000_0000],
        [0x4060_3C00, 0x8000_0000],
        [0x40F0_3C03, 0],
        [0x40B0_0700, u32::MAX],
        [0x40C0_0001, 0x0500_0102],
        [0x40D0_0000, 0x1234_5678],
        [0x40E0_0000, 0x8000_0000],
        [0x4020_0000, 0x1800_0000],
        [0x4030_0205, 1],
        [0x3016_0102, 0x0304_0506],
        [0x10F8_0000, 0],
        [0x3032_0708, 0],
        [0x4080_3C00, 0x8000_0000],
    ]
    .into_iter()
    .map(|[first, second]| Event::new(0, [first, second, 0, 0]))
    .collect();
    let mut decoder = Decoder::new();
    let seen = assert_no_allocations("packets decoded", || {
        let mut seen = 0;
        for _ in 0..4 {
            for packet in &packets {
                for message in decoder.decode(packet) {
                    if let Message::SysEx { data, .. } = message {
                        seen += data.len();
                    }
                    seen += 1;
                }
            }
        }
        seen
    });
    // Each round: twelve packets of one message each, the note-off's two,
    // for the two notes on its key, and the SysEx once whole with its eight
    // bytes; its first part and the clock are none.
    assert_eq!(seen, 4 * (12 + 2 + 1 + 8));
}

#[test]
fn reading_a_block_and_sending_does_not_allocate() {
    let mut packets = EventList::with_capacity(16);
    for (offset, words) in [
        (0, [0x4090_3C00, 0xFFFF_0000]),
        (3, [0x40B0_0700, u32::MAX]),
        (5, [0x3016_0102, 0x0304_0506]),
        (6, [0x3032_0708, 0]),
        (9, [0x4080_3C00, 0x8000_0000]),
    ] {
        packets.push(Event::new(offset, [words[0], words[1], 0, 0]));
    }
    let mut list = MidiList::new();
    let mut sent = EventList::with_capacity(64);
    let seen = assert_no_allocations("a block read and sent", || {
        let mut seen = 0;
        for _ in 0..4 {
            sent.clear();
            list.read(&packets, 16);
            let midi = list.midi();
            for part in [midi, midi.between(0, 8), midi.between(8, 16)] {
                let mut out = MidiOut::new(&mut sent);
                for (frame, message) in part {
                    out.send(frame, message);
                    seen += 1;
                }
                out.extend(part.packets());
            }
            MidiOut::new(&mut sent).send(0, NoteOn::new(Key::C4, 1.0));
        }
        seen
    });
    // The block's four messages, whole, then split two and two.
    assert_eq!(seen, 4 * 8);
}
