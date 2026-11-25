//! Fuzzes upgrading a host's MIDI 1.0, cut into blocks anywhere, as an
//! exporter does, then downgrading it for a MIDI 1.0-only destination and
//! upgrading it again. It checks these properties:
//! - Nothing panics, and nothing is dropped or lost.
//! - Every upgraded packet has a MIDI 1.0 origin.
//! - The second upgrade gives the first one's packets at the same frames,
//!   however the blocks split a data entry that the upgrade holds.
#![no_main]

use hapto_ump::{Downgrade, Event, EventList, Midi1Message, Midi1Voice, Origin, Upgrade};
use libfuzzer_sys::fuzz_target;

/// The frame at which a block's held high bytes are sent.
const LAST_FRAME: u32 = 63;

/// Builds blocks of messages from `data`, four bytes to a message:
/// - The first byte's top bit starts a block, and its low bits pick the group.
/// - The second byte's high bits pick the channel, and its low bits pick the
///   kind, most often a control change.
/// - The last two bytes are the data.
fn blocks(data: &[u8]) -> Vec<Vec<Event>> {
    let mut blocks = vec![Vec::new()];
    for &[head, kind, a, b] in data.as_chunks::<4>().0 {
        if head & 0x80 != 0 {
            blocks.push(Vec::new());
        }
        let (a, b) = (a & 0x7F, b & 0x7F);
        let message = match kind & 0x07 {
            0 => Midi1Message::NoteOff {
                note: a,
                velocity: b,
            },
            1 => Midi1Message::NoteOn {
                note: a,
                velocity: b,
            },
            2 => Midi1Message::PolyPressure { note: a, value: b },
            3 => Midi1Message::ProgramChange { program: a },
            4 => Midi1Message::ChannelPressure { value: a },
            5 => Midi1Message::PitchBend {
                value: u16::from(b) << 7 | u16::from(a),
            },
            _ => Midi1Message::ControlChange {
                controller: a,
                value: b,
            },
        };
        let block = blocks.last_mut().expect("one block at least");
        let frame = u32::try_from(block.len()).map_or(LAST_FRAME, |at| at.min(LAST_FRAME));
        let voice = Midi1Voice {
            group: head & 0x0F,
            channel: kind >> 4,
            message,
        };
        block.push(voice.event(frame));
    }
    blocks
}

/// Upgrades `sent` one block at a time with one `Upgrade`.
fn upgrade(sent: &[Vec<Event>]) -> Vec<Vec<Event>> {
    let mut upgrade = Upgrade::new();
    sent.iter()
        .map(|block| {
            // Room for each message and a held high byte per group and channel.
            let mut out = EventList::with_capacity(block.len() + 256);
            upgrade.block(block, LAST_FRAME, &mut out);
            assert_eq!(out.dropped(), 0);
            out.as_slice().to_vec()
        })
        .collect()
}

/// Downgrades `upgraded` one block at a time.
fn downgrade(upgraded: &[Vec<Event>]) -> Vec<Vec<Event>> {
    let mut downgrade = Downgrade::new();
    let back = upgraded
        .iter()
        .map(|block| {
            // A program change with its bank becomes three messages, and a
            // controller becomes four.
            let mut out = EventList::with_capacity(block.len() * 4);
            downgrade.block(block, &mut out);
            assert_eq!(out.dropped(), 0);
            out.as_slice().to_vec()
        })
        .collect();
    assert_eq!(downgrade.lost(), 0, "MIDI 1.0 carries all an upgrade makes");
    back
}

/// Returns each block's packets and frames, without their origins.
fn packets(blocks: &[Vec<Event>]) -> Vec<Vec<(u32, [u32; 4])>> {
    blocks
        .iter()
        .map(|block| {
            block
                .iter()
                .map(|event| (event.offset, event.words))
                .collect()
        })
        .collect()
}

fuzz_target!(|data: &[u8]| {
    let upgraded = upgrade(&blocks(data));
    assert!(
        upgraded
            .iter()
            .flatten()
            .all(|event| matches!(event.origin, Origin::Midi1 | Origin::Midi1HighByte))
    );
    let again = upgrade(&downgrade(&upgraded));
    assert_eq!(packets(&again), packets(&upgraded));
});
