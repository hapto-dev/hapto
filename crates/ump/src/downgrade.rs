//! MIDI 2.0 downgraded to MIDI 1.0 on the way out, by the Default
//! Translation in appendix D.2 of the UMP and MIDI 2.0 Protocol
//! specification.

use crate::scale::{down_16_to_7, down_32_to_7, down_32_to_14};
use crate::{
    Attribute, Bank, ChannelMessage, ChannelVoice, Event, EventList, Midi1Message, Midi1Voice,
};

/// Downgrades MIDI 2.0 to MIDI 1.0, for a destination that takes only MIDI
/// 1.0: when someone asks for it, or the other end forces it.
///
/// It follows the specification's Default Translation. Values cut their
/// low bits off. A note-on that would cut to velocity 0, which MIDI 1.0
/// reads as a note-off, goes out at velocity 1. A registered or assignable
/// controller becomes its four controllers (101 and 100, or 99 and 98, then
/// 6 and 38), and a program change with a bank becomes bank select (0 and
/// 32) and then the program.
///
/// What MIDI 1.0 cannot carry is dropped and counted in
/// [`lost`](Self::lost): relative controllers, per-note controllers, per-note
/// pitch bend and per-note management, and a note's attribute, whose note
/// still goes out. Other packets pass as they are.
///
/// # Examples
///
/// ```
/// use hapto_ump::{Attribute, ChannelMessage, ChannelVoice, Downgrade, EventList, Midi1Message, Midi1Voice};
///
/// let mut downgrade = Downgrade::new();
/// let quiet = ChannelVoice {
///     group: 0,
///     channel: 0,
///     message: ChannelMessage::NoteOn { note: 60, velocity: 0x0100, attribute: Attribute::NONE },
/// };
/// let mut out = EventList::with_capacity(4);
/// downgrade.block(&[quiet.event(0)], &mut out);
///
/// // Still a note-on in MIDI 1.0: velocity 1, not the 0 that ends a note.
/// let sent = out.as_slice()[0].midi1_voice().map(|voice| voice.message);
/// assert_eq!(sent, Some(Midi1Message::NoteOn { note: 60, velocity: 1 }));
/// assert_eq!(downgrade.lost(), 0);
/// ```
#[derive(Debug, Default)]
pub struct Downgrade {
    lost: u64,
}

impl Downgrade {
    /// A downgrade that has lost nothing yet.
    pub const fn new() -> Self {
        Downgrade { lost: 0 }
    }

    /// What it has dropped because MIDI 1.0 cannot carry it, since it was
    /// made.
    pub const fn lost(&self) -> u64 {
        self.lost
    }

    /// Downgrades a block's packets, in order, into `out`. A packet past
    /// `out`'s room is dropped and counted there.
    ///
    /// Thread: audio. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn block(&mut self, input: &[Event], out: &mut EventList) {
        for event in input {
            self.event(event, out);
        }
    }

    /// Downgrades one packet into `out`: a MIDI 2.0 channel voice message
    /// becomes the MIDI 1.0 messages it translates to, and any other packet
    /// passes as it is.
    ///
    /// Thread: audio. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn event(&mut self, event: &Event, out: &mut EventList) {
        let Some(ChannelVoice {
            group,
            channel,
            message,
        }) = event.channel_voice()
        else {
            out.push(*event);
            return;
        };
        let send = |message: Midi1Message, out: &mut EventList| {
            out.push(
                Midi1Voice {
                    group,
                    channel,
                    message,
                }
                .event(event.offset),
            );
        };
        let cc = |controller: u8, value: u8| Midi1Message::ControlChange { controller, value };
        match message {
            ChannelMessage::NoteOn {
                note,
                velocity,
                attribute,
            } => {
                self.count_attribute(attribute);
                let velocity = down_16_to_7(velocity).max(1);
                send(Midi1Message::NoteOn { note, velocity }, out);
            }
            ChannelMessage::NoteOff {
                note,
                velocity,
                attribute,
            } => {
                self.count_attribute(attribute);
                let velocity = down_16_to_7(velocity);
                send(Midi1Message::NoteOff { note, velocity }, out);
            }
            ChannelMessage::PolyPressure { note, value } => send(
                Midi1Message::PolyPressure {
                    note,
                    value: down_32_to_7(value),
                },
                out,
            ),
            ChannelMessage::ControlChange { controller, value } => {
                send(cc(controller, down_32_to_7(value)), out);
            }
            ChannelMessage::ProgramChange { program, bank } => {
                if let Some(Bank { msb, lsb }) = bank {
                    send(cc(0, msb), out);
                    send(cc(32, lsb), out);
                }
                send(Midi1Message::ProgramChange { program }, out);
            }
            ChannelMessage::ChannelPressure { value } => send(
                Midi1Message::ChannelPressure {
                    value: down_32_to_7(value),
                },
                out,
            ),
            ChannelMessage::PitchBend { value } => send(
                Midi1Message::PitchBend {
                    value: down_32_to_14(value),
                },
                out,
            ),
            ChannelMessage::RegisteredController { bank, index, value }
            | ChannelMessage::AssignableController { bank, index, value } => {
                let (high, low) = if matches!(message, ChannelMessage::RegisteredController { .. })
                {
                    (101, 100)
                } else {
                    (99, 98)
                };
                let value = down_32_to_14(value);
                send(cc(high, bank), out);
                send(cc(low, index), out);
                send(cc(6, (value >> 7) as u8), out);
                send(cc(38, (value & 0x7F) as u8), out);
            }
            _ => self.lost = self.lost.saturating_add(1),
        }
    }

    /// Counts a note's attribute, which MIDI 1.0 has nowhere to carry.
    fn count_attribute(&mut self, attribute: Attribute) {
        if attribute != Attribute::NONE {
            self.lost = self.lost.saturating_add(1);
        }
    }
}
