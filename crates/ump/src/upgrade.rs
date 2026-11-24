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

/// Upgrades MIDI 1.0 to MIDI 2.0, as an exporter does on the way in.
///
/// It follows the specification's Default Translation. Values upscale as
/// [`scale`](crate::scale) says, and every packet
/// it makes is marked [`Origin::Midi1`], or [`Origin::Midi1HighByte`] for
/// a data entry high byte sent with no low byte. A note-on at velocity 0
/// becomes a note-off at `0x8000`. The pair of a 14-bit controller stays two
/// controllers. Bank select is held for the next program change, which
/// carries it, and is not translated on its own; a bank byte never sent
/// reads as 0.
///
/// An RPN or NRPN becomes one registered or assignable controller. Its
/// data entry high byte (controller 6) goes out at once, at its own frame,
/// as if its low byte were 0. Its low byte (38) at the same frame joins it,
/// so a pair sent together is one message: the upgrade rewrites the packet
/// it just sent, while that is still the last in the list. A low byte at a
/// later frame refines it as a message of its own. Nothing waits for a
/// later message, so what comes out never depends on where the blocks are
/// cut. An address may come in either byte order, and the null number
/// (127, 127) selects nothing. Reset All Controllers forgets the address.
///
/// A data entry belongs to the number it was sent under. A low byte refines
/// its number's latest high byte however often the address is sent between,
/// as the specification holds the latest high byte. The upgrade departs
/// from the specification in two places, so that one parameter's high byte
/// never lands on another: a low byte never takes the high byte of another
/// number, and one sent before its own number's high byte waits for it.
///
/// Other packets pass as they are. State is kept for every group and
/// channel, made once and never grown, so [`block`](Self::block) allocates
/// nothing.
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
/// // NRPN 2, 5 at 100, sent at frame 1, then its low byte moved alone.
/// let sent = [cc(99, 2), cc(98, 5), cc(6, 100), cc(38, 0)].map(|cc| cc.event(1));
/// let mut upgraded = EventList::with_capacity(4);
/// upgrade.block(&sent, &mut upgraded);
/// upgrade.block(&[cc(38, 1).event(5)], &mut upgraded);
///
/// // One MIDI 2.0 message for the pair sent at one frame, and one for the
/// // low byte that refines it later.
/// let [pair, fine] = upgraded.as_slice() else { panic!("two messages") };
/// assert_eq!((pair.offset, pair.origin), (1, Origin::Midi1));
/// assert_eq!((fine.offset, fine.origin), (5, Origin::Midi1));
/// let message = pair.channel_voice().map(|voice| voice.message);
/// assert!(matches!(message, Some(ChannelMessage::AssignableController { bank: 2, index: 5, .. })));
/// ```
#[derive(Debug)]
pub struct Upgrade {
    channels: Box<[[Channel; 16]; 16]>,
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
    /// The latest data entry high byte, already sent. A low byte under its
    /// number refines it, however often the address is sent again; one
    /// under another number never does.
    high: Option<Held>,
    /// A low byte that arrived before its number's high byte, which MIDI 1.0
    /// allows, kept for that high byte.
    early_low: Option<(Number, u8)>,
}

/// A data entry high byte, with the number it was sent under.
#[derive(Debug, Clone, Copy)]
struct Held {
    number: Number,
    value: u8,
}

/// A parameter number's kind, bank and index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Number {
    kind: Kind,
    bank: u8,
    index: u8,
}

/// A data entry to send: its number, its high byte, and its low byte unless
/// none came.
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

    /// An address byte, 99 or 98 for an NRPN, 101 or 100 for an RPN.
    ///
    /// A pair comes in either order (JUCE's generator sends the low byte
    /// first), so a byte completes the address with its partner when that
    /// one is still unpaired. A high byte with nothing to pair with starts a
    /// new address and forgets the low byte held, which belonged to an
    /// address already used. A low byte with nothing to pair with changes
    /// the low byte under the high byte held. 127 and 127 select nothing.
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

    /// A control change to a parameter-number controller or Reset All
    /// Controllers, sending what it completes.
    fn control(&mut self, controller: u8, value: u8, send: &mut Sender<'_>) {
        match controller {
            NRPN_LSB..=RPN_MSB => self.address_byte(controller, value),
            DATA_ENTRY => {
                self.unpaired = None;
                // A low byte sent first is spent here, even by a data entry
                // that selects nothing.
                let early = self.early_low.take();
                let Some(number) = self.number() else {
                    self.high = None;
                    return;
                };
                self.high = Some(Held { number, value });
                let low = early
                    .filter(|&(under, _)| under == number)
                    .map(|(_, low)| low);
                send.entry(Entry {
                    number,
                    high: value,
                    low,
                });
            }
            DATA_ENTRY_LSB => {
                self.unpaired = None;
                let Some(number) = self.number() else {
                    return;
                };
                match self.high {
                    Some(held) if held.number == number => send.refine(Entry {
                        number,
                        high: held.value,
                        low: Some(value),
                    }),
                    _ => self.early_low = Some((number, value)),
                }
            }
            RESET_ALL_CONTROLLERS => *self = Numbers::default(),
            // Increment and decrement: an address byte before one pairs with
            // no byte after it.
            _ => self.unpaired = None,
        }
    }
}

impl Channel {
    /// Translates one MIDI 1.0 message on this channel, sending each MIDI
    /// 2.0 message it completes.
    fn translate(&mut self, message: Midi1Message, send: &mut Sender<'_>) {
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
        send.send(translated);
    }

    /// Translates a MIDI 1.0 control change: bank select is held, the
    /// parameter-number controllers feed their state, and the rest become
    /// MIDI 2.0 control changes.
    fn control_change(&mut self, controller: u8, value: u8, send: &mut Sender<'_>) {
        let as_is = ChannelMessage::ControlChange {
            controller,
            value: up_7_to_32(value),
        };
        match controller {
            BANK_SELECT => self.bank[0] = Some(value),
            BANK_SELECT_LSB => self.bank[1] = Some(value),
            _ if is_parameter_number(controller) || controller == RESET_ALL_CONTROLLERS => {
                self.numbers.control(controller, value, send);
                // Increment and decrement are controllers of their own in
                // MIDI 2.0, with no parameter number to step, and Reset All
                // Controllers resets the rest.
                if matches!(
                    controller,
                    DATA_INCREMENT | DATA_DECREMENT | RESET_ALL_CONTROLLERS
                ) {
                    send.send(as_is);
                }
            }
            _ => send.send(as_is),
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
        }
    }

    /// Upgrades a block's packets, in order, into `out`. A packet past
    /// `out`'s room is dropped and counted there.
    ///
    /// Thread: audio. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn block(&mut self, input: &[Event], out: &mut EventList) {
        for event in input {
            self.event(event, out);
        }
    }

    /// Upgrades one packet into `out`: a MIDI 1.0 channel voice message
    /// becomes what it completes, which may be nothing, and any other packet
    /// passes as it is. A data entry low byte may rewrite the packet `out`
    /// ends with, its high byte sent at the same frame, as [`Upgrade`] says.
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
        let state = self
            .channels
            .get_mut(usize::from(group))
            .and_then(|channels| channels.get_mut(usize::from(channel)));
        let Some(state) = state else {
            return;
        };
        let mut send = Sender {
            out,
            group,
            channel,
            offset: event.offset,
        };
        state.translate(message, &mut send);
    }
}

/// Where one MIDI 1.0 message's translation goes: `out`, on its group and
/// channel, at its frame.
struct Sender<'a> {
    out: &'a mut EventList,
    group: u8,
    channel: u8,
    offset: u32,
}

impl Sender<'_> {
    /// The packet `message` is here, marked with `origin`.
    fn event(&self, message: ChannelMessage, origin: Origin) -> Event {
        let voice = ChannelVoice {
            group: self.group,
            channel: self.channel,
            message,
        };
        Event {
            origin,
            ..voice.event(self.offset)
        }
    }

    /// Sends a message translated whole from MIDI 1.0.
    fn send(&mut self, message: ChannelMessage) {
        self.out.push(self.event(message, Origin::Midi1));
    }

    /// Sends a data entry as it is.
    fn entry(&mut self, entry: Entry) {
        let (message, origin) = entry.message();
        self.out.push(self.event(message, origin));
    }

    /// Sends a data entry its low byte completes. When the packet last sent
    /// is its high byte alone, at this frame, the pair rewrites it, so a pair
    /// sent at one frame is one message; otherwise the pair refines the high
    /// byte as a message of its own.
    fn refine(&mut self, entry: Entry) {
        let (message, origin) = Entry { low: None, ..entry }.message();
        let alone = self.event(message, origin);
        let (message, origin) = entry.message();
        let pair = self.event(message, origin);
        match self.out.as_mut_slice().last_mut() {
            Some(last) if *last == alone => *last = pair,
            _ => self.out.push(pair),
        }
    }
}

impl Entry {
    /// The MIDI 2.0 controller it becomes, with how it was sent: with its low
    /// byte, or as a high byte alone, which goes out as if its low byte were
    /// 0.
    fn message(self) -> (ChannelMessage, Origin) {
        let Number { kind, bank, index } = self.number;
        let value = up_14_to_32(join_14(self.low.unwrap_or(0), self.high));
        let origin = if self.low.is_some() {
            Origin::Midi1
        } else {
            Origin::Midi1HighByte
        };
        (kind.message(bank, index, value), origin)
    }
}
