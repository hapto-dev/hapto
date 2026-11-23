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
/// It follows the specification's Default Translation. Values scale up as
/// [`scale`](crate::scale) describes, and each packet it makes is marked
/// [`Origin::Midi1`], or [`Origin::Midi1HighByte`] for a data entry high byte
/// sent without a low byte. Other packets pass through unchanged.
///
/// - A note-on at velocity 0 becomes a note-off at velocity `0x8000`.
/// - A 14-bit controller pair stays two controllers.
/// - Bank select (0 and 32) is held for the next program change, which
///   carries it. A bank byte that was never sent counts as 0.
/// - An RPN or NRPN becomes one registered or assignable controller.
/// - Data increment and decrement (96, 97) pass on as controllers of their
///   own. They name no number, so a destination of the downgrade steps the
///   number that it last saw.
///
/// The data entry high byte (6) goes out at once, at its own frame, as if its
/// low byte were 0. Nothing waits for a later message, so the output never
/// depends on where the blocks are cut.
///
/// - A low byte (38) at the same frame joins the high byte, so a pair sent
///   together is one message. The upgrade rewrites the packet that it just
///   sent, while that packet is still the last in the list.
/// - A low byte at a later frame refines the high byte as a second message.
///
/// Each address byte (99 and 98, or 101 and 100) replaces the byte that it
/// names and keeps the other, because the specification holds the latest of
/// the four. So an address can come in either byte order, a sender can send
/// only the byte that changed, and the null number (127, 127) selects nothing.
/// Reset All Controllers clears the address.
///
/// A data entry belongs to the number that it was sent under. A low byte
/// refines its own number's latest high byte, even if the address is sent
/// again in between, because the specification holds the latest high byte.
/// The upgrade differs from the specification in three places. The first two
/// keep one parameter's high byte from reaching another:
///
/// - A low byte never combines with another number's high byte.
/// - A low byte sent before its own number's high byte waits for it, until
///   the next address byte.
/// - Bank select also passes on as controllers 0 and 32 when it comes. The
///   specification carries it only in the program change and tells a MIDI 2.0
///   receiver to ignore the controllers, but common controllers send a fader
///   or a button on them, which a plugin can learn. Sent back through
///   [`Downgrade`](crate::Downgrade), MIDI 1.0 then repeats its bank select
///   before each program change, which leaves a receiver on the same bank.
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
    /// The kind that data entry applies to, which is that of the latest
    /// address byte unless the null number is selected.
    selected: Option<Kind>,
    /// The latest data entry high byte, already sent. Only a low byte under
    /// the same number refines it, however often the address is sent again.
    high: Option<Held>,
    /// A low byte that arrived before its number's high byte, which MIDI 1.0
    /// allows, kept for that high byte unless an address byte comes first.
    /// It always belongs to the selected number, because only an address
    /// byte or Reset All Controllers changes the number, and each drops it.
    early_low: Option<u8>,
}

/// A data entry high byte, with the number that it was sent under.
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

/// A data entry to send, with its number, its high byte and its low byte,
/// if one arrived.
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

    /// Records an address byte, 99 or 98 for an NRPN and 101 or 100 for an
    /// RPN. It replaces the byte that it names, keeps the other, and selects
    /// its kind.
    fn address_byte(&mut self, controller: u8, value: u8) {
        let (kind, most) = Kind::of_address(controller);
        let bytes = self.address(kind);
        let [msb, lsb] = &mut *bytes;
        *(if most { msb } else { lsb }) = Some(value);
        let null = *bytes == [Some(NULL), Some(NULL)];
        self.selected = (!null).then_some(kind);
    }

    /// Handles a parameter-number control change or Reset All Controllers,
    /// and sends the controller that the change completes.
    fn control(&mut self, controller: u8, value: u8, send: &mut Sender<'_>) {
        match controller {
            NRPN_LSB..=RPN_MSB => {
                // A low byte sent first pairs with the high byte sent right
                // after its address, never with one a later address sends:
                // a MIDI 1.0 receiver would hold that one with a low byte
                // of 0.
                self.early_low = None;
                self.address_byte(controller, value);
            }
            DATA_ENTRY => {
                // A data entry consumes `early_low`, which belongs to the
                // selected number, because every address byte and Reset All
                // Controllers drop it, so a data entry that selects nothing
                // never finds one.
                let low = self.early_low.take();
                let Some(number) = self.number() else {
                    self.high = None;
                    return;
                };
                self.high = Some(Held { number, value });
                send.entry(Entry {
                    number,
                    high: value,
                    low,
                });
            }
            DATA_ENTRY_LSB => {
                let Some(number) = self.number() else {
                    return;
                };
                match self.high {
                    Some(held) if held.number == number => send.refine(Entry {
                        number,
                        high: held.value,
                        low: Some(value),
                    }),
                    _ => self.early_low = Some(value),
                }
            }
            RESET_ALL_CONTROLLERS => *self = Numbers::default(),
            // Increment and decrement step the value a receiver holds, which
            // the upgrade does not.
            _ => {}
        }
    }
}

impl Channel {
    /// Translates one MIDI 1.0 message on this channel, and sends each
    /// completed MIDI 2.0 message.
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

    /// Translates a MIDI 1.0 control change by passing parameter-number
    /// controllers to `Numbers` and sending the rest as MIDI 2.0 control
    /// changes. Bank select is sent and also held.
    fn control_change(&mut self, controller: u8, value: u8, send: &mut Sender<'_>) {
        let as_is = ChannelMessage::ControlChange {
            controller,
            value: up_7_to_32(value),
        };
        match controller {
            BANK_SELECT => {
                self.bank[0] = Some(value);
                send.send(as_is);
            }
            BANK_SELECT_LSB => {
                self.bank[1] = Some(value);
                send.send(as_is);
            }
            _ if is_parameter_number(controller) || controller == RESET_ALL_CONTROLLERS => {
                self.numbers.control(controller, value, send);
                // In MIDI 2.0, increment and decrement are controllers of
                // their own with no parameter number to step, and Reset All
                // Controllers resets the other controllers, so all three also
                // go out as control changes.
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

    /// Forgets everything held, as when a new source takes over the stream:
    /// a high byte, an address, a bank. What was held is not sent.
    ///
    /// Thread: any. Allocates nothing.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_ump::{EventList, Midi1Message, Midi1Voice, Upgrade};
    ///
    /// let cc = |controller, value| {
    ///     let message = Midi1Message::ControlChange { controller, value };
    ///     Midi1Voice { group: 0, channel: 0, message }.event(0)
    /// };
    /// let mut upgrade = Upgrade::new();
    /// let mut out = EventList::with_capacity(4);
    /// // An NRPN's address, then a new source sends its low byte alone.
    /// upgrade.block(&[cc(99, 2), cc(98, 5)], 63, &mut out);
    /// upgrade.reset();
    /// upgrade.block(&[cc(6, 64)], 63, &mut out);
    /// assert!(out.as_slice().is_empty(), "no address: nothing to send");
    /// ```
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn reset(&mut self) {
        for group in self.channels.iter_mut() {
            group.fill(Channel::default());
        }
    }

    /// Upgrades a block's packets, in order, into `out`. Any high byte that is
    /// still held then goes out at `last_frame`.
    ///
    /// A packet that doesn't fit is dropped, and `out` counts it.
    ///
    /// Thread: audio. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn block(&mut self, input: &[Event], out: &mut EventList) {
        for event in input {
            self.event(event, out);
        }
    }

    /// Upgrades one packet into `out`.
    ///
    /// A MIDI 1.0 channel voice message becomes the MIDI 2.0 messages that it
    /// completes, if any, and any other packet passes through unchanged. A
    /// data entry low byte may rewrite the packet that `out` ends with, when
    /// that packet is its high byte sent at the same frame, as [`Upgrade`]
    /// describes. So `out` is the block's own list, as [`block`](Self::block)
    /// says.
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
    /// Returns this entry's MIDI 2.0 controller, with an origin that says
    /// whether it had a low byte.
    ///
    /// A high byte alone goes out as if its low byte were 0.
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
