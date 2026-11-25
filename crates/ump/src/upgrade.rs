//! Upgrades incoming MIDI 1.0 to MIDI 2.0 with the Default Translation in
//! appendix D.3 of the UMP and MIDI 2.0 Protocol specification.

use crate::controller::{
    BANK_SELECT, BANK_SELECT_LSB, DATA_DECREMENT, DATA_ENTRY, DATA_ENTRY_LSB, DATA_INCREMENT,
    NRPN_LSB, NRPN_MSB, RESET_ALL_CONTROLLERS, RPN_LSB, RPN_MSB, is_parameter_number,
};
use crate::head::join_14;
use crate::scale::{up_7_to_16, up_7_to_32, up_14_to_32};
use crate::{
    Attribute, Bank, ChannelMessage, ChannelVoice, Event, EventList, Midi1Message, Midi1Voice,
    Origin,
};

/// Both bytes of the null parameter number, which selects nothing.
const NULL: u8 = 127;

/// Upgrades incoming MIDI 1.0 to MIDI 2.0, as an exporter does.
///
/// Values scale up as [`scale`](crate::scale) describes, and each packet it
/// makes is marked [`Origin::Midi1`]. Other packets pass through unchanged.
///
/// - A note-on at velocity 0 becomes a note-off at velocity `0x8000`.
/// - A 14-bit controller pair stays two controllers.
/// - Bank select (0 and 32) is held for the next program change, which
///   carries it. A bank byte that was never sent counts as 0.
/// - An RPN or NRPN becomes one registered or assignable controller. Its
///   address can come in either byte order, the null number (127, 127)
///   selects nothing, and Reset All Controllers clears it.
///
/// The data entry high byte (6) is held, and the controller goes out when one
/// of these comes:
///
/// - The data entry low byte (38).
/// - Another high byte, or an address byte.
/// - A data increment or decrement (96, 97), which goes out after it.
/// - The end of the block. The controller then goes out at the block's last
///   frame, which is the short timeout that the specification allows, so a
///   sender that never sends the low byte loses nothing.
///
/// A controller sent without its low byte is marked
/// [`Origin::Midi1HighByte`].
///
/// A held byte belongs to the number that it was sent under. A low byte
/// refines its own number's latest high byte, even if the address is sent
/// again in between, as the specification says. The upgrade differs from the
/// specification in two places, so that one parameter's high byte never
/// reaches another:
///
/// - A low byte never combines with another number's high byte.
/// - A low byte sent before its own number's high byte waits for it.
///
/// State for every group and channel is allocated once, so
/// [`block`](Self::block) allocates nothing.
///
/// # Examples
///
/// ```
/// use hapto_ump::{ChannelMessage, EventList, Midi1Message, Midi1Voice, Origin, Upgrade};
///
/// let mut upgrade = Upgrade::new();
/// let cc = |controller, value| Midi1Voice {
///     group: 0,
///     channel: 0,
///     message: Midi1Message::ControlChange { controller, value },
/// };
/// // These four MIDI 1.0 controllers set NRPN 2, 5 to 100.
/// let sent = [cc(99, 2).event(0), cc(98, 5).event(0), cc(6, 100).event(1), cc(38, 0).event(2)];
/// let mut upgraded = EventList::with_capacity(4);
/// upgrade.block(&sent, 63, &mut upgraded);
///
/// // They become one MIDI 2.0 message, at the low byte's frame.
/// let [event] = upgraded.as_slice() else { panic!("one message") };
/// assert_eq!((event.offset, event.origin), (2, Origin::Midi1));
/// let message = event.channel_voice().map(|voice| voice.message);
/// assert!(matches!(message, Some(ChannelMessage::AssignableController { bank: 2, index: 5, .. })));
/// ```
#[derive(Debug)]
pub struct Upgrade {
    channels: Box<[[Channel; 16]; 16]>,
    /// For each group, a bit for each channel that holds an unsent high byte,
    /// so that `block` checks only those channels when the block ends.
    pending: [u16; 16],
}

/// The MIDI 1.0 parameter number and bank of one group and channel.
#[derive(Debug, Clone, Copy, Default)]
struct Channel {
    numbers: Numbers,
    /// Bank select's two bytes, controllers 0 and 32, held for the next
    /// program change.
    bank: [Option<u8>; 2],
}

/// Whether a parameter number is registered (an RPN in MIDI 1.0) or
/// assignable (an NRPN).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Registered,
    Assignable,
}

impl Kind {
    /// Returns the kind that an address byte selects, and whether it is the
    /// most significant byte.
    const fn of_address(controller: u8) -> (Kind, bool) {
        match controller {
            RPN_MSB => (Kind::Registered, true),
            RPN_LSB => (Kind::Registered, false),
            NRPN_MSB => (Kind::Assignable, true),
            _ => (Kind::Assignable, false),
        }
    }

    /// Returns this kind's two address controllers, the most significant
    /// first.
    const fn address(self) -> [u8; 2] {
        match self {
            Kind::Registered => [RPN_MSB, RPN_LSB],
            Kind::Assignable => [NRPN_MSB, NRPN_LSB],
        }
    }

    /// Returns this kind's MIDI 2.0 controller at `bank` and `index`.
    const fn message(self, bank: u8, index: u8, value: u32) -> ChannelMessage {
        match self {
            Kind::Registered => ChannelMessage::RegisteredController { bank, index, value },
            Kind::Assignable => ChannelMessage::AssignableController { bank, index, value },
        }
    }
}

/// A channel's RPN and NRPN state.
#[derive(Debug, Clone, Copy, Default)]
struct Numbers {
    /// An NRPN's address bytes, 99 and 98.
    nrpn: [Option<u8>; 2],
    /// An RPN's address bytes, 101 and 100.
    rpn: [Option<u8>; 2],
    /// An unpaired address byte, sent since the last data entry or completed
    /// address.
    unpaired: Option<u8>,
    /// The kind that data entry applies to, if an address selects one.
    selected: Option<Kind>,
    /// The latest data entry high byte, which only a low byte under the same
    /// number refines.
    high: Option<Held>,
    /// A low byte that arrived before its number's high byte, which MIDI 1.0
    /// allows, kept for that high byte.
    early_low: Option<(Number, u8)>,
}

/// A held data entry high byte, with the number that it was sent under.
#[derive(Debug, Clone, Copy)]
struct Held {
    number: Number,
    value: u8,
    /// Whether it is unsent, because no low byte has arrived.
    pending: bool,
}

/// A parameter number's kind, bank and index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Number {
    kind: Kind,
    bank: u8,
    index: u8,
}

/// A data entry ready to send, with its number, its high byte and its low
/// byte, if one arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    number: Number,
    high: u8,
    low: Option<u8>,
}

impl Numbers {
    /// Returns the selected number, if its address is complete.
    fn number(&self) -> Option<Number> {
        let kind = self.selected?;
        let bytes = match kind {
            Kind::Registered => self.rpn,
            Kind::Assignable => self.nrpn,
        };
        let [Some(bank), Some(index)] = bytes else {
            return None;
        };
        Some(Number { kind, bank, index })
    }

    /// Returns the address bytes held for `kind`.
    fn address(&mut self, kind: Kind) -> &mut [Option<u8>; 2] {
        match kind {
            Kind::Registered => &mut self.rpn,
            Kind::Assignable => &mut self.nrpn,
        }
    }

    /// Returns whether a held high byte is still pending.
    fn is_pending(&self) -> bool {
        self.high.is_some_and(|held| held.pending)
    }

    /// Sends the pending high byte without a low byte, and clears `pending`.
    fn flush(&mut self, send: &mut impl FnMut(ChannelMessage, Origin)) {
        if let Some(held) = self.high.as_mut().filter(|held| held.pending) {
            held.pending = false;
            Entry {
                number: held.number,
                high: held.value,
                low: None,
            }
            .send(send);
        }
    }

    /// Records an address byte, 99 or 98 for an NRPN and 101 or 100 for an
    /// RPN.
    ///
    /// A pair can come in either order, and JUCE's generator sends the low
    /// byte first.
    ///
    /// - A byte whose partner is unpaired completes the address.
    /// - An unpaired high byte starts a new address and clears the held low
    ///   byte, which belonged to an address already used.
    /// - An unpaired low byte replaces the one that goes with the held high
    ///   byte.
    /// - The pair 127, 127 selects nothing.
    fn address_byte(&mut self, controller: u8, value: u8) {
        let (kind, most) = Kind::of_address(controller);
        let [msb, lsb] = kind.address();
        let partner = if most { lsb } else { msb };
        let pairs = self.unpaired == Some(partner);
        let bytes = self.address(kind);
        if most {
            let low = if pairs { bytes[1] } else { None };
            *bytes = [Some(value), low];
        } else {
            bytes[1] = Some(value);
        }
        let null = *bytes == [Some(NULL), Some(NULL)];
        self.unpaired = (!pairs).then_some(controller);
        self.selected = (!null).then_some(kind);
    }

    /// Handles a parameter-number control change or Reset All Controllers. It
    /// first sends any held controller that the change ends, then the one
    /// that the change completes.
    fn control(
        &mut self,
        controller: u8,
        value: u8,
        send: &mut impl FnMut(ChannelMessage, Origin),
    ) {
        match controller {
            NRPN_LSB..=RPN_MSB => {
                // A new address sends the held high byte first.
                self.flush(send);
                self.address_byte(controller, value);
            }
            DATA_ENTRY => {
                self.unpaired = None;
                // Another high byte sends the previous one first.
                self.flush(send);
                // A data entry consumes `early_low`, even when it selects
                // nothing.
                let early = self.early_low.take();
                let Some(number) = self.number() else {
                    self.high = None;
                    return;
                };
                let complete = matches!(early, Some((under, _)) if under == number);
                self.high = Some(Held {
                    number,
                    value,
                    pending: !complete,
                });
                if let Some((_, low)) = early.filter(|_| complete) {
                    Entry {
                        number,
                        high: value,
                        low: Some(low),
                    }
                    .send(send);
                }
            }
            DATA_ENTRY_LSB => {
                self.unpaired = None;
                let Some(number) = self.number() else {
                    return;
                };
                match self.high.as_mut() {
                    Some(held) if held.number == number => {
                        held.pending = false;
                        Entry {
                            number,
                            high: held.value,
                            low: Some(value),
                        }
                        .send(send);
                    }
                    _ => self.early_low = Some((number, value)),
                }
            }
            RESET_ALL_CONTROLLERS => {
                self.flush(send);
                *self = Numbers::default();
            }
            // Increment and decrement step the parameter, so a held high byte
            // goes out first, in the order sent. An address byte sent before
            // them doesn't pair with any byte after them.
            _ => {
                self.unpaired = None;
                self.flush(send);
            }
        }
    }
}

impl Channel {
    /// Translates one MIDI 1.0 message on this channel, and sends each
    /// completed MIDI 2.0 message with its origin.
    fn translate(&mut self, message: Midi1Message, mut send: impl FnMut(ChannelMessage, Origin)) {
        let translated = match message {
            Midi1Message::NoteOn { note, velocity: 0 } => ChannelMessage::NoteOff {
                note,
                velocity: 0x8000,
                attribute: Attribute::NONE,
            },
            Midi1Message::NoteOn { note, velocity } => ChannelMessage::NoteOn {
                note,
                velocity: up_7_to_16(velocity),
                attribute: Attribute::NONE,
            },
            Midi1Message::NoteOff { note, velocity } => ChannelMessage::NoteOff {
                note,
                velocity: up_7_to_16(velocity),
                attribute: Attribute::NONE,
            },
            Midi1Message::PolyPressure { note, value } => ChannelMessage::PolyPressure {
                note,
                value: up_7_to_32(value),
            },
            Midi1Message::ChannelPressure { value } => ChannelMessage::ChannelPressure {
                value: up_7_to_32(value),
            },
            Midi1Message::PitchBend { value } => ChannelMessage::PitchBend {
                value: up_14_to_32(value),
            },
            Midi1Message::ProgramChange { program } => {
                // Either byte alone selects a bank, with the other at 0.
                let bank = match self.bank {
                    [None, None] => None,
                    [msb, lsb] => Some(Bank {
                        msb: msb.unwrap_or(0),
                        lsb: lsb.unwrap_or(0),
                    }),
                };
                ChannelMessage::ProgramChange { program, bank }
            }
            Midi1Message::ControlChange { controller, value } => {
                self.control_change(controller, value, send);
                return;
            }
        };
        send(translated, Origin::Midi1);
    }

    /// Translates a MIDI 1.0 control change by holding bank select, passing
    /// parameter-number controllers to `Numbers`, and sending the rest as MIDI
    /// 2.0 control changes.
    fn control_change(
        &mut self,
        controller: u8,
        value: u8,
        mut send: impl FnMut(ChannelMessage, Origin),
    ) {
        let as_is = ChannelMessage::ControlChange {
            controller,
            value: up_7_to_32(value),
        };
        match controller {
            BANK_SELECT => self.bank[0] = Some(value),
            BANK_SELECT_LSB => self.bank[1] = Some(value),
            _ if is_parameter_number(controller) || controller == RESET_ALL_CONTROLLERS => {
                self.numbers.control(controller, value, &mut send);
                // In MIDI 2.0, increment and decrement are controllers of
                // their own with no parameter number to step, and Reset All
                // Controllers resets the other controllers, so all three also
                // go out as control changes.
                if matches!(
                    controller,
                    DATA_INCREMENT | DATA_DECREMENT | RESET_ALL_CONTROLLERS
                ) {
                    send(as_is, Origin::Midi1);
                }
            }
            _ => send(as_is, Origin::Midi1),
        }
    }
}

impl Default for Upgrade {
    fn default() -> Self {
        Upgrade::new()
    }
}

impl Upgrade {
    /// Returns an upgrade with no state yet.
    ///
    /// It allocates, so an exporter creates it at activation.
    ///
    /// Thread: any but audio.
    pub fn new() -> Self {
        Upgrade {
            channels: Box::new([[Channel::default(); 16]; 16]),
            pending: [0; 16],
        }
    }

    /// Upgrades a block's packets, in order, into `out`. Any high byte that is
    /// still held then goes out at `last_frame`.
    ///
    /// A packet that doesn't fit is dropped, and `out` counts it.
    ///
    /// Thread: audio. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn block(&mut self, input: &[Event], last_frame: u32, out: &mut EventList) {
        for event in input {
            self.event(event, out);
        }
        self.end_block(last_frame, out);
    }

    /// Upgrades one packet into `out`.
    ///
    /// A MIDI 1.0 channel voice message becomes the MIDI 2.0 messages that it
    /// completes, if any, and any other packet passes through unchanged.
    ///
    /// Thread: audio. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn event(&mut self, event: &Event, out: &mut EventList) {
        let Some(Midi1Voice {
            group,
            channel,
            message,
        }) = Midi1Voice::decode(event.words)
        else {
            out.push(*event);
            return;
        };
        let (Some(channels), Some(pending)) = (
            self.channels.get_mut(usize::from(group)),
            self.pending.get_mut(usize::from(group)),
        ) else {
            return;
        };
        let Some(state) = channels.get_mut(usize::from(channel)) else {
            return;
        };
        state.translate(message, sender(out, group, channel, event.offset));
        let bit = 1 << channel;
        if state.numbers.is_pending() {
            *pending |= bit;
        } else {
            *pending &= !bit;
        }
    }

    /// Sends every held high byte at `last_frame`.
    ///
    /// The end of a block is the timeout after which a data entry without a
    /// low byte goes out alone.
    ///
    /// Thread: audio. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn end_block(&mut self, last_frame: u32, out: &mut EventList) {
        let groups = self.channels.iter_mut().zip(&mut self.pending);
        for (group, (channels, pending)) in (0_u8..).zip(groups) {
            if *pending == 0 {
                continue;
            }
            let held = std::mem::take(pending);
            let channels = (0_u8..).zip(channels.iter_mut());
            for (channel, state) in channels.filter(|(channel, _)| held & (1 << channel) != 0) {
                state
                    .numbers
                    .flush(&mut sender(out, group, channel, last_frame));
            }
        }
    }
}

/// Returns a callback that pushes each MIDI 2.0 message, with its origin,
/// onto `out` on `group` and `channel` at frame `offset`.
fn sender(
    out: &mut EventList,
    group: u8,
    channel: u8,
    offset: u32,
) -> impl FnMut(ChannelMessage, Origin) {
    move |message, origin| {
        let voice = ChannelVoice {
            group,
            channel,
            message,
        };
        out.push(Event {
            origin,
            ..voice.event(offset)
        });
    }
}

impl Entry {
    /// Sends this entry's MIDI 2.0 controller, with an origin that says
    /// whether it had a low byte.
    ///
    /// A high byte alone goes out as if its low byte were 0.
    fn send(self, send: &mut impl FnMut(ChannelMessage, Origin)) {
        let Number { kind, bank, index } = self.number;
        let value = up_14_to_32(join_14(self.low.unwrap_or(0), self.high));
        let origin = if self.low.is_some() {
            Origin::Midi1
        } else {
            Origin::Midi1HighByte
        };
        send(kind.message(bank, index, value), origin);
    }
}
