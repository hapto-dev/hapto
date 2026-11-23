//! From messages back to MIDI 2.0 packets: what a block sends.

// Audio-path code: a panic here ends a host, so indexing, unwrap, expect,
// panic and unchecked arithmetic are refused, and a site that is provably
// safe says why in an `#[expect]`.
#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects
    )
)]

use hapto_ump::scale::up_14_to_32;
use hapto_ump::{Attribute, Bank, ChannelMessage, ChannelVoice};

use crate::message::{Message, NoteKey};
use crate::values::{Centred, Channel, Group, Pitch};

/// MIDI 2.0's registered per-note controller for a note's absolute pitch.
const PER_NOTE_PITCH: u8 = 3;

/// Hands `send` the packets `message` is as MIDI 2.0: one for most, as many
/// as a SysEx needs, six bytes each.
pub(crate) fn encode(message: &Message<'_>, mut send: impl FnMut([u32; 4])) {
    if let Message::SysEx { group, data } = *message {
        sysex(group, data, send);
        return;
    }
    if let Some(voice) = voice(message) {
        send(voice.words());
    }
}

fn voice(message: &Message<'_>) -> Option<ChannelVoice> {
    let on = |group: Group, channel: Channel, message| ChannelVoice {
        group: group.index() as u8,
        channel: channel.index() as u8,
        message,
    };
    let per_note = |note: NoteKey, message| on(note.group, note.channel, message);
    Some(match *message {
        Message::NoteOn(note) => on(
            note.group,
            note.channel,
            ChannelMessage::NoteOn {
                note: note.key.number(),
                velocity: sixteen(note.velocity),
                attribute: note.attribute,
            },
        ),
        Message::NoteOff(off) => on(
            off.group,
            off.channel,
            ChannelMessage::NoteOff {
                note: off.key.number(),
                velocity: sixteen(off.velocity),
                attribute: Attribute::NONE,
            },
        ),
        Message::NotePitch { note, pitch } => per_note(
            note,
            ChannelMessage::RegisteredPerNoteController {
                note: note.key.number(),
                index: PER_NOTE_PITCH,
                value: pitch_7_25(pitch),
            },
        ),
        Message::NoteBend { note, value } => per_note(
            note,
            ChannelMessage::PerNotePitchBend {
                note: note.key.number(),
                value: centred(value),
            },
        ),
        Message::NotePressure { note, value } => per_note(
            note,
            ChannelMessage::PolyPressure {
                note: note.key.number(),
                value: bits(value),
            },
        ),
        Message::NoteControl {
            note,
            controller,
            value,
        } => per_note(
            note,
            ChannelMessage::RegisteredPerNoteController {
                note: note.key.number(),
                index: controller,
                value: bits(value),
            },
        ),
        Message::NoteAssignable { note, index, value } => per_note(
            note,
            ChannelMessage::AssignablePerNoteController {
                note: note.key.number(),
                index,
                value: bits(value),
            },
        ),
        Message::NoteManagement {
            note,
            detach,
            reset,
        } => per_note(
            note,
            ChannelMessage::PerNoteManagement {
                note: note.key.number(),
                detach,
                reset,
            },
        ),
        _ => return channel_wide(message),
    })
}

fn channel_wide(message: &Message<'_>) -> Option<ChannelVoice> {
    let (group, channel, message) = match *message {
        Message::Bend {
            group,
            channel,
            value,
        } => (
            group,
            channel,
            ChannelMessage::PitchBend {
                value: centred(value),
            },
        ),
        Message::BendRange {
            group,
            channel,
            semitones,
        } => (
            group,
            channel,
            ChannelMessage::RegisteredController {
                bank: 0,
                index: 0,
                value: bend_range(semitones),
            },
        ),
        Message::Pressure {
            group,
            channel,
            value,
        } => (
            group,
            channel,
            ChannelMessage::ChannelPressure { value: bits(value) },
        ),
        Message::Control {
            group,
            channel,
            controller,
            value,
        } => (
            group,
            channel,
            ChannelMessage::ControlChange {
                controller,
                value: bits(value),
            },
        ),
        Message::Registered {
            group,
            channel,
            bank,
            index,
            value,
        } => (
            group,
            channel,
            ChannelMessage::RegisteredController {
                bank,
                index,
                value: bits(value),
            },
        ),
        Message::Assignable {
            group,
            channel,
            bank,
            index,
            value,
        } => (
            group,
            channel,
            ChannelMessage::AssignableController {
                bank,
                index,
                value: bits(value),
            },
        ),
        Message::Program {
            group,
            channel,
            program,
            bank,
        } => (
            group,
            channel,
            ChannelMessage::ProgramChange {
                program: program.wire(),
                bank: bank.map(|bank| Bank {
                    msb: ((bank >> 7) & 0x7F) as u8,
                    lsb: (bank & 0x7F) as u8,
                }),
            },
        ),
        _ => return None,
    };
    Some(ChannelVoice {
        group: group.index() as u8,
        channel: channel.index() as u8,
        message,
    })
}

/// A SysEx's bytes as 7-bit SysEx packets: one whole, or a first, middle
/// ones and a last, six bytes each.
fn sysex(group: Group, data: &[u8], mut send: impl FnMut([u32; 4])) {
    let group = group.index() as u32;
    let last = data.len().saturating_sub(1) / 6;
    let mut chunks = data.chunks(6).enumerate().peekable();
    if chunks.peek().is_none() {
        send(sysex7(group, 0x0, &[]));
    }
    for (index, chunk) in chunks {
        let status = match (index, last) {
            (0, 0) => 0x0,
            (0, _) => 0x1,
            (i, l) if i == l => 0x3,
            _ => 0x2,
        };
        send(sysex7(group, status, chunk));
    }
}

fn sysex7(group: u32, status: u32, chunk: &[u8]) -> [u32; 4] {
    let mut bytes = [0_u32; 6];
    for (slot, &byte) in bytes.iter_mut().zip(chunk) {
        *slot = u32::from(byte & 0x7F);
    }
    let [b1, b2, b3, b4, b5, b6] = bytes;
    let count = chunk.len().min(6) as u32;
    [
        0x3 << 28 | (group & 0xF) << 24 | status << 20 | count << 16 | b1 << 8 | b2,
        b3 << 24 | b4 << 16 | b5 << 8 | b6,
        0,
        0,
    ]
}

/// A value from 0 to 1 as 32 bits, held to the ends.
fn bits(value: f32) -> u32 {
    (f64::from(value.clamp(0.0, 1.0)) * f64::from(u32::MAX)).round() as u32
}

/// A value from 0 to 1 as 16 bits, held to the ends.
fn sixteen(value: f32) -> u16 {
    (value.clamp(0.0, 1.0) * f32::from(u16::MAX)).round() as u16
}

/// A centred value as MIDI 2.0's 32 bits, its centre at `0x8000_0000`.
fn centred(value: Centred) -> u32 {
    const CENTRE: f64 = 2_147_483_648.0;
    let value = f64::from(value.get());
    let bits = if value >= 0.0 {
        CENTRE + value * (f64::from(u32::MAX) - CENTRE)
    } else {
        CENTRE + value * CENTRE
    };
    bits.round() as u32
}

/// A pitch as MIDI 2.0's 7.25.
fn pitch_7_25(pitch: Pitch) -> u32 {
    let semitones = f64::from(pitch.semitones()).clamp(0.0, 127.999_999);
    (semitones * f64::from(1_u32 << 25)).round() as u32
}

/// A bend range in semitones as registered parameter 0's value: semitones
/// and cents, as MIDI 1.0 sends them, upscaled as the specification does.
fn bend_range(semitones: f32) -> u32 {
    // In whole cents, so 1.999 carries to 2 semitones, and none for NaN.
    let cents = if semitones.is_nan() {
        0
    } else {
        (semitones.clamp(0.0, 127.99) * 100.0).round() as u16
    };
    let (whole, cents) = (
        cents.checked_div(100).unwrap_or(0),
        cents.checked_rem(100).unwrap_or(0),
    );
    up_14_to_32(whole << 7 | cents)
}
