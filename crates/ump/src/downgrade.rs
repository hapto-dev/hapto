//! Downgrades outgoing MIDI 2.0 to MIDI 1.0 with the Default Translation in
//! appendix D.2 of the UMP and MIDI 2.0 Protocol specification.

use crate::controller::{
    BANK_SELECT, BANK_SELECT_LSB, DATA_ENTRY, DATA_ENTRY_LSB, NRPN_LSB, NRPN_MSB, RPN_LSB, RPN_MSB,
};
use crate::head::split_14;
use crate::scale::{down_16_to_7, down_32_to_7, down_32_to_14};
use crate::{
    Attribute, Bank, Bytes, ChannelMessage, ChannelVoice, Event, EventList, Midi1Message,
    Midi1Voice,
};

/// Downgrades MIDI 2.0 to MIDI 1.0 for a destination that accepts only MIDI
/// 1.0, when someone asks for it or the other end forces it.
///
/// It follows the specification's Default Translation. Values drop their low
/// bits, and three messages change form.
///
/// What comes out is what MIDI 1.0 carries, exactly what [`Bytes::of`]
/// writes: its channel voice messages, its system messages and 7-bit SysEx.
/// The rest is dropped, and each call returns how many packets it dropped:
/// relative controllers, per-note controllers, per-note pitch bend and
/// per-note management, a note's attribute, whose note still goes out, and
/// any packet MIDI 1.0 has no bytes for, such as a utility or stream
/// message, 8-bit SysEx, or a status MIDI 1.0 leaves undefined.
///
/// # Examples
///
/// ```
/// use hapto_ump::{Attribute, ChannelMessage, ChannelVoice, Downgrade, EventList, Midi1Message, Midi1Voice};
///
/// let quiet = ChannelVoice {
///     group: 0,
///     channel: 0,
///     message: ChannelMessage::NoteOn { note: 60, velocity: 0x0100, attribute: Attribute::NONE },
/// };
/// let mut out = EventList::with_capacity(4);
/// let lost = Downgrade::new().block(&[quiet.event(0)], &mut out);
///
/// // It stays a note-on, at velocity 1 rather than the 0 that ends a note.
/// let sent = out.as_slice()[0].midi1_voice().map(|voice| voice.message);
/// assert_eq!(sent, Some(Midi1Message::NoteOn { note: 60, velocity: 1 }));
/// assert_eq!(lost, 0);
/// ```
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct Downgrade {}

impl Downgrade {
    /// The Default Translation's downgrade.
    pub const fn new() -> Self {
        Downgrade {}
    }

    /// Downgrades a block's packets, in order, into `out`, and returns how
    /// many MIDI 1.0 could not carry. A packet past `out`'s room is dropped
    /// and counted there.
    ///
    /// Thread: audio. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn block(&self, input: &[Event], out: &mut EventList) -> u32 {
        input.iter().fold(0, |lost, event| {
            lost.saturating_add(self.translate(event, |packet| out.push(packet)))
        })
    }

    /// Downgrades one packet, handing `send` each MIDI 1.0 packet it
    /// becomes, and returns how many MIDI 1.0 could not carry: none, or one
    /// for a packet dropped or a note's attribute.
    ///
    /// Thread: audio. Allocates nothing.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_ump::{Downgrade, Event};
    ///
    /// // MIDI 2.0's controller 7 at full, as MIDI 1.0's at 127.
    /// let mut sent = Vec::new();
    /// let controller = Event::new(5, [0x40B0_0700, u32::MAX, 0, 0]);
    /// let lost = Downgrade::new().translate(&controller, |packet| sent.push(packet));
    /// assert_eq!((sent, lost), (vec![Event::new(5, [0x20B0_077F, 0, 0, 0])], 0));
    ///
    /// // A utility message has no MIDI 1.0 bytes.
    /// let lost = Downgrade::new().translate(&Event::new(0, [0; 4]), |_| unreachable!());
    /// assert_eq!(lost, 1);
    /// ```
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn translate(&self, event: &Event, mut send: impl FnMut(Event)) -> u32 {
        let Some(ChannelVoice {
            group,
            channel,
            message,
        }) = event.channel_voice()
        else {
            // Any other packet goes as it is, if MIDI 1.0 has bytes for it.
            if Bytes::of(event.words).is_none() {
                return 1;
            }
            send(*event);
            return 0;
        };
        let mut send = |message: Midi1Message| {
            send(
                Midi1Voice {
                    group,
                    channel,
                    message,
                }
                .event(event.offset),
            );
        };
        match message {
            ChannelMessage::NoteOn {
                note,
                velocity,
                attribute,
            } => {
                let velocity = down_16_to_7(velocity).max(1);
                send(Midi1Message::NoteOn { note, velocity });
                attribute_lost(attribute)
            }
            ChannelMessage::NoteOff {
                note,
                velocity,
                attribute,
            } => {
                let velocity = down_16_to_7(velocity);
                send(Midi1Message::NoteOff { note, velocity });
                attribute_lost(attribute)
            }
            ChannelMessage::PolyPressure { note, value } => {
                send(Midi1Message::PolyPressure {
                    note,
                    value: down_32_to_7(value),
                });
                0
            }
            ChannelMessage::ControlChange { controller, value } => {
                send(cc(controller, down_32_to_7(value)));
                0
            }
            ChannelMessage::ProgramChange { program, bank } => {
                if let Some(Bank { msb, lsb }) = bank {
                    send(cc(BANK_SELECT, msb));
                    send(cc(BANK_SELECT_LSB, lsb));
                }
                send(Midi1Message::ProgramChange { program });
                0
            }
            ChannelMessage::ChannelPressure { value } => {
                send(Midi1Message::ChannelPressure {
                    value: down_32_to_7(value),
                });
                0
            }
            ChannelMessage::PitchBend { value } => {
                send(Midi1Message::PitchBend {
                    value: down_32_to_14(value),
                });
                0
            }
            ChannelMessage::RegisteredController { bank, index, value } => {
                number(&mut send, [RPN_MSB, RPN_LSB], [bank, index], value);
                0
            }
            ChannelMessage::AssignableController { bank, index, value } => {
                number(&mut send, [NRPN_MSB, NRPN_LSB], [bank, index], value);
                0
            }
            _ => 1,
        }
    }
}

/// A MIDI 1.0 control change.
const fn cc(controller: u8, value: u8) -> Midi1Message {
    Midi1Message::ControlChange { controller, value }
}

/// Sends a registered or assignable controller as MIDI 1.0's four: its
/// `address` controllers with its bank and index, then data entry's high
/// and low bytes.
fn number(
    send: &mut impl FnMut(Midi1Message),
    [msb, lsb]: [u8; 2],
    [bank, index]: [u8; 2],
    value: u32,
) {
    let (low, high) = split_14(down_32_to_14(value));
    send(cc(msb, bank));
    send(cc(lsb, index));
    send(cc(DATA_ENTRY, high));
    send(cc(DATA_ENTRY_LSB, low));
}

/// One for a note's attribute, which MIDI 1.0 has nowhere to carry.
fn attribute_lost(attribute: Attribute) -> u32 {
    u32::from(attribute != Attribute::NONE)
}
