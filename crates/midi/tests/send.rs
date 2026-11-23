//! What a plugin sends: a note moved to another key with its pitch, a note
//! of its own beside the host's, a SysEx whole or not at all, the bend
//! range at its edges, and a controller that could never read refused.

use hapto_midi::controller::Controller;
use hapto_midi::{Channel, Decoder, Group, Key, Message, MidiOut, NoteId, NoteOn};
use hapto_ump::{Event, EventList};

/// What `send` sent, decoded again, each message's line.
fn sent(room: usize, send: impl FnOnce(&mut MidiOut<'_>)) -> (Vec<String>, u32) {
    let mut packets = EventList::with_capacity(room);
    let mut out = MidiOut::new(&mut packets);
    send(&mut out);
    let dropped = out.dropped();
    let mut decoder = Decoder::new();
    let lines = packets
        .as_slice()
        .iter()
        .flat_map(|event| {
            decoder
                .decode(event)
                .map(|m| m.to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    (lines, dropped)
}

/// The note-on `words` decodes to.
fn note_on(words: [u32; 2]) -> NoteOn {
    let event = Event::new(0, [words[0], words[1], 0, 0]);
    match Decoder::new().decode(&event).next() {
        Some(Message::NoteOn(on)) => on,
        other => panic!("not a note-on: {other:?}"),
    }
}

#[test]
fn a_note_moved_to_another_key_moves_its_own_pitch_with_it() {
    // A4 a quarter tone sharp, in MIDI 2.0's pitch attribute: 7.9 fixed
    // point, a key 512.
    let a4 = note_on([0x4090_4503, 0xFFFF_0000 | 69 << 9 | 0x100]);
    assert_eq!(a4.pitch().semitones(), 69.5);
    let e5 = a4.with_key(Key::new(76));
    assert_eq!(
        (e5.key, e5.pitch().semitones(), e5.id),
        (Key::new(76), 76.5, a4.id)
    );
    // It goes out with its pitch moved too.
    let (lines, _) = sent(4, |out| out.send(0, e5));
    let sounded = lines.first().cloned().unwrap_or_default();
    assert!(sounded.contains("E5"), "{sounded}");
    // A note with no pitch of its own sounds at its key.
    let plain = NoteOn::new(Key::C4, 1.0).with_key(Key::new(48));
    assert_eq!(plain.pitch().semitones(), 48.0);
}

#[test]
fn a_note_of_the_plugins_own_never_meets_a_hosts_or_a_made_up_one() {
    let made_up = note_on([0x4090_3C00, 0xFFFF_0000]).id;
    let own = NoteId::own(made_up.get() & 0x3FFF_FFFF);
    assert!(!own.is_hosts());
    assert_ne!(own, made_up);
    assert_ne!(own, NoteId::hosts(own.get()));
    assert_eq!(NoteId::own(3).to_string(), "plugin's note 3");
    // A chord's fifth, beside the host's note, under an id of its own.
    let root = NoteOn::new(Key::C4, 1.0).with_id(NoteId::hosts(7));
    let fifth = root.with_key(Key::new(67)).with_id(NoteId::own(1));
    assert_ne!(root.id, fifth.id);
}

#[test]
fn a_sysex_goes_out_whole_or_not_at_all() {
    let (group, data) = (Group::FIRST, [0x7D_u8; 20]);
    // Four packets of six bytes, with room for three.
    let (lines, dropped) = sent(3, |out| out.send(0, Message::SysEx { group, data: &data }));
    assert_eq!((lines.len(), dropped), (0, 4), "nothing half sent");
    // Room for all four.
    let (lines, dropped) = sent(4, |out| out.send(0, Message::SysEx { group, data: &data }));
    assert_eq!((lines.len(), dropped), (1, 0));
}

#[test]
fn a_bend_range_carries_its_cents_and_a_nan_is_none() {
    let range = |semitones: f32| {
        let message = Message::BendRange {
            group: Group::FIRST,
            channel: Channel::FIRST,
            semitones,
        };
        let mut packets = EventList::with_capacity(4);
        MidiOut::new(&mut packets).send(0, message);
        match Decoder::new().decode(&packets.as_slice()[0]).next() {
            Some(Message::BendRange { semitones, .. }) => semitones,
            other => panic!("not a bend range: {other:?}"),
        }
    };
    assert_eq!(range(12.5), 12.5);
    assert_eq!(range(1.999), 2.0, "100 cents carry to the next semitone");
    assert_eq!(range(f32::NAN), 0.0);
    assert_eq!(range(500.0), 127.99, "held to the most MIDI carries");
}

#[test]
#[should_panic(expected = "the pitch bend's range")]
fn a_registered_controller_on_the_bend_range_is_refused() {
    let _never_reads = Controller::<f32>::registered(0, 0, 0.0);
}
