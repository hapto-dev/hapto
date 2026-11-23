//! From messages back to MIDI 2.0 packets: what a block sends.

use hapto_ump::scale::{self, up_14_to_32};
use hapto_ump::{Attribute, Bank, ChannelMessage, ChannelVoice, SysEx7};

use crate::message::Message;
use crate::values::{Centred, Pitch, held};

/// Hands `send` the packets `message` is as MIDI 2.0: one for most, as many
/// as a SysEx needs, six bytes each.
pub(crate) fn encode(message: &Message<'_>, mut send: impl FnMut([u32; 4])) {
    let group = message.group().wire();
    if let Message::SysEx { data, .. } = *message {
        SysEx7::packets(group, data, send);
        return;
    }
    let (Some(channel), Some(voice)) = (
        message.channel(),
        note_message(message).or_else(|| channel_message(message)),
    ) else {
        return;
    };
    let voice = ChannelVoice {
        group,
        channel: channel.wire(),
        message: voice,
    };
    send(voice.words());
}

/// A note's message as MIDI 2.0's; `None` for a channel's.
fn note_message(message: &Message<'_>) -> Option<ChannelMessage> {
    Some(match *message {
        Message::NoteOn(on) => ChannelMessage::NoteOn {
            note: on.key.wire(),
            velocity: sixteen(on.velocity),
            attribute: on.attribute,
        },
        Message::NoteOff(off) => ChannelMessage::NoteOff {
            note: off.key.wire(),
            velocity: sixteen(off.velocity),
            attribute: Attribute::NONE,
        },
        Message::NotePitch { note, pitch } => ChannelMessage::RegisteredPerNoteController {
            note: note.key.wire(),
            index: ChannelMessage::PER_NOTE_PITCH,
            value: pitch_7_25(pitch),
        },
        Message::NoteBend { note, value } => ChannelMessage::PerNotePitchBend {
            note: note.key.wire(),
            value: centred(value),
        },
        Message::NotePressure { note, value } => ChannelMessage::PolyPressure {
            note: note.key.wire(),
            value: bits(value),
        },
        Message::NoteControl {
            note,
            controller,
            value,
        } => ChannelMessage::RegisteredPerNoteController {
            note: note.key.wire(),
            index: controller,
            value: bits(value),
        },
        Message::NoteAssignable { note, index, value } => {
            ChannelMessage::AssignablePerNoteController {
                note: note.key.wire(),
                index,
                value: bits(value),
            }
        }
        Message::NoteManagement {
            note,
            detach,
            reset,
        } => ChannelMessage::PerNoteManagement {
            note: note.key.wire(),
            detach,
            reset,
        },
        _ => return None,
    })
}

/// A channel's message as MIDI 2.0's; `None` for a note's or a SysEx.
fn channel_message(message: &Message<'_>) -> Option<ChannelMessage> {
    Some(match *message {
        Message::Bend { value, .. } => ChannelMessage::PitchBend {
            value: centred(value),
        },
        Message::BendRange { semitones, .. } => ChannelMessage::RegisteredController {
            bank: 0,
            index: 0,
            value: bend_range(semitones),
        },
        Message::Pressure { value, .. } => ChannelMessage::ChannelPressure { value: bits(value) },
        Message::Control {
            controller, value, ..
        } => ChannelMessage::ControlChange {
            controller,
            value: bits(value),
        },
        Message::Registered {
            bank, index, value, ..
        } => ChannelMessage::RegisteredController {
            bank,
            index,
            value: bits(value),
        },
        Message::Assignable {
            bank, index, value, ..
        } => ChannelMessage::AssignableController {
            bank,
            index,
            value: bits(value),
        },
        Message::Program { program, bank, .. } => ChannelMessage::ProgramChange {
            program: program.wire(),
            bank: bank.map(|bank| Bank {
                msb: ((bank >> 7) & 0x7F) as u8,
                lsb: (bank & 0x7F) as u8,
            }),
        },
        _ => return None,
    })
}

/// A value from 0 to 1 as 32 bits, held to the ends.
fn bits(value: f32) -> u32 {
    scale::unit_to_32(f64::from(value))
}

/// A value from 0 to 1 as 16 bits, held to the ends.
fn sixteen(value: f32) -> u16 {
    scale::unit_to_16(f64::from(value))
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
    scale::pitch_7_25(f64::from(pitch.semitones()))
}

/// A bend range in semitones as registered parameter 0's value: semitones
/// and cents, as MIDI 1.0 sends them, upscaled as the specification does.
fn bend_range(semitones: f32) -> u32 {
    // In whole cents, so 1.999 carries to 2 semitones, and none for NaN.
    let cents = (held(semitones, 0.0, 127.99, 0.0) * 100.0).round() as u16;
    let (whole, cents) = (
        cents.checked_div(100).unwrap_or(0),
        cents.checked_rem(100).unwrap_or(0),
    );
    up_14_to_32(whole << 7 | cents)
}
