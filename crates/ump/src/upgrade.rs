//! MIDI 1.0 upgraded to MIDI 2.0 on the way in, by the Default Translation
//! in appendix D.3 of the UMP and MIDI 2.0 Protocol specification.

use crate::head::join_14;
use crate::scale::{up_7_to_16, up_7_to_32, up_14_to_32};
use crate::{
    Attribute, Bank, ChannelMessage, ChannelVoice, Event, EventList, Midi1Message, Midi1Voice,
    Origin,
};

/// MIDI 1.0's Reset All Controllers, which also sets the parameter number to
/// null (RP-015).
const RESET_ALL_CONTROLLERS: u8 = 121;
/// The null parameter number's two bytes: it selects nothing.
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
/// data entry high byte (controller 6) is held, and the controller goes out
/// when the low byte (38) arrives, when another high byte arrives, when an
/// address byte arrives, or before data increment or decrement (96, 97). A
/// high byte still held at the end of a block goes out at the block's last
/// frame: the short timeout the specification allows, so a sender that
/// never sends the low byte loses nothing. An address may come in either
/// byte order, and the null number (127, 127) selects nothing. Reset All
/// Controllers forgets the address.
///
/// A held byte belongs to the number it was sent under. A low byte refines
/// its number's high byte however often the address is sent between, as
/// the specification holds the latest high byte. The upgrade departs from
/// the specification in two places, so that one parameter's high byte never
/// lands on another: a low byte never takes the high byte of another number,
/// and one sent before its own number's high byte waits for it.
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
/// // NRPN 2, 5 at 100: four MIDI 1.0 controllers.
/// let sent = [cc(99, 2).event(0), cc(98, 5).event(0), cc(6, 100).event(1), cc(38, 0).event(2)];
/// let mut upgraded = EventList::with_capacity(4);
/// upgrade.block(&sent, 63, &mut upgraded);
///
/// // One MIDI 2.0 message, at the low byte's frame.
/// let [event] = upgraded.as_slice() else { panic!("one message") };
/// assert_eq!((event.offset, event.origin), (2, Origin::Midi1));
/// let message = event.channel_voice().map(|voice| voice.message);
/// assert!(matches!(message, Some(ChannelMessage::AssignableController { bank: 2, index: 5, .. })));
/// ```
#[derive(Debug)]
pub struct Upgrade {
    channels: Box<[[Channel; 16]; 16]>,
}

/// One group and channel's MIDI 1.0 state: its parameter number and its
/// bank.
#[derive(Debug, Clone, Copy, Default)]
struct Channel {
    numbers: Numbers,
    /// Bank select's two bytes, controllers 0 and 32, held for the next
    /// program change.
    bank: [Option<u8>; 2],
}

/// Which parameter number data entry is for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Selected {
    #[default]
    None,
    Nrpn,
    Rpn,
}

/// A channel's RPN and NRPN state.
#[derive(Debug, Clone, Copy, Default)]
struct Numbers {
    /// Controllers 99 and 98.
    nrpn: [Option<u8>; 2],
    /// Controllers 101 and 100.
    rpn: [Option<u8>; 2],
    /// The address byte sent since the last data entry or completed
    /// address: its partner completes the address with it.
    unpaired: Option<u8>,
    selected: Selected,
    /// The latest data entry high byte, with the number it was sent under.
    /// A low byte under that number refines it, however often its address
    /// is sent again; one under another number never does.
    high: Option<(Number, u8)>,
    /// A low byte that arrived before its number's high byte, which MIDI 1.0
    /// allows; that high byte completes it.
    early_low: Option<(Number, u8)>,
    /// Whether `high` is still to go out.
    pending: bool,
}

/// A parameter number: registered (an RPN) or assignable (an NRPN), with its
/// bank and index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Number {
    registered: bool,
    bank: u8,
    index: u8,
}

/// A data entry ready to go out: its number, its high byte, and its low
/// byte unless none came.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    number: Number,
    high: u8,
    low: Option<u8>,
}

impl Numbers {
    /// The selected number, when its address is whole.
    fn number(&self) -> Option<Number> {
        let (registered, bytes) = match self.selected {
            Selected::None => return None,
            Selected::Nrpn => (false, self.nrpn),
            Selected::Rpn => (true, self.rpn),
        };
        let [Some(bank), Some(index)] = bytes else {
            return None;
        };
        Some(Number {
            registered,
            bank,
            index,
        })
    }

    /// The high byte still to go out, with no low byte, which leaves it no
    /// longer pending.
    fn flush(&mut self) -> Option<Entry> {
        if !std::mem::take(&mut self.pending) {
            return None;
        }
        let (number, high) = self.high?;
        Some(Entry {
            number,
            high,
            low: None,
        })
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
        let partner = match controller {
            99 => 98,
            98 => 99,
            101 => 100,
            _ => 101,
        };
        let pairs = self.unpaired == Some(partner);
        let (bytes, selected) = if matches!(controller, 99 | 98) {
            (&mut self.nrpn, Selected::Nrpn)
        } else {
            (&mut self.rpn, Selected::Rpn)
        };
        if matches!(controller, 99 | 101) {
            if !pairs {
                bytes[1] = None;
            }
            bytes[0] = Some(value);
        } else {
            bytes[1] = Some(value);
        }
        let null = *bytes == [Some(NULL), Some(NULL)];
        self.unpaired = (!pairs).then_some(controller);
        self.selected = if null { Selected::None } else { selected };
    }

    /// A control change to a parameter-number controller, and what it sends
    /// out, up to two entries: the one it ends, then its own.
    fn control(&mut self, controller: u8, value: u8) -> [Option<Entry>; 2] {
        match controller {
            98..=101 => {
                // A new address ends the message the last high byte began.
                let ended = self.flush();
                self.address_byte(controller, value);
                [ended, None]
            }
            6 => {
                self.unpaired = None;
                // Another high byte ends the last one's message.
                let ended = self.flush();
                // A low byte sent first is spent here, even by a data entry
                // that selects nothing.
                let early = self.early_low.take();
                let Some(number) = self.number() else {
                    self.high = None;
                    return [ended, None];
                };
                self.high = Some((number, value));
                let own = match early {
                    Some((under, low)) if under == number => Some(Entry {
                        number,
                        high: value,
                        low: Some(low),
                    }),
                    _ => {
                        self.pending = true;
                        None
                    }
                };
                [ended, own]
            }
            38 => {
                self.unpaired = None;
                let Some(number) = self.number() else {
                    return [None, None];
                };
                match self.high {
                    Some((under, high)) if under == number => {
                        self.pending = false;
                        [
                            Some(Entry {
                                number,
                                high,
                                low: Some(value),
                            }),
                            None,
                        ]
                    }
                    _ => {
                        self.early_low = Some((number, value));
                        [None, None]
                    }
                }
            }
            // Increment and decrement (96, 97) step the parameter, so a high
            // byte held goes out first, in the order sent. An address byte
            // held before one pairs with no byte after it.
            _ => {
                self.unpaired = None;
                [self.flush(), None]
            }
        }
    }
}

impl Channel {
    /// Translates one MIDI 1.0 message on this channel, sending each MIDI
    /// 2.0 message it completes with its origin.
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
                // Either byte alone selects a bank, its partner at 0.
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

    /// Translates a MIDI 1.0 control change: bank select is held, the
    /// parameter-number controllers feed their state, and the rest become
    /// MIDI 2.0 control changes.
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
            0 => self.bank[0] = Some(value),
            32 => self.bank[1] = Some(value),
            6 | 38 | 96..=101 => {
                for entry in self
                    .numbers
                    .control(controller, value)
                    .into_iter()
                    .flatten()
                {
                    send(entry.message(), entry.origin());
                }
                // Increment and decrement are controllers of their own in
                // MIDI 2.0, with no parameter number to step.
                if matches!(controller, 96 | 97) {
                    send(as_is, Origin::Midi1);
                }
            }
            _ => {
                if controller == RESET_ALL_CONTROLLERS {
                    if let Some(entry) = self.numbers.flush() {
                        send(entry.message(), entry.origin());
                    }
                    self.numbers = Numbers::default();
                }
                send(as_is, Origin::Midi1);
            }
        }
    }
}

impl Default for Upgrade {
    fn default() -> Self {
        Upgrade::new()
    }
}

impl Upgrade {
    /// An upgrade with no state yet. Allocates, so an exporter makes it at
    /// activation.
    ///
    /// Thread: any but audio.
    pub fn new() -> Self {
        Upgrade {
            channels: Box::new([[Channel::default(); 16]; 16]),
        }
    }

    /// Upgrades a block's packets, in order, into `out`, then sends any
    /// high byte still held at `last_frame`, the block's last. A packet past
    /// `out`'s room is dropped and counted there.
    ///
    /// Thread: audio. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn block(&mut self, input: &[Event], last_frame: u32, out: &mut EventList) {
        for event in input {
            self.event(event, out);
        }
        self.end_block(last_frame, out);
    }

    /// Upgrades one packet into `out`: a MIDI 1.0 channel voice message
    /// becomes what it completes, which may be nothing yet, and any other
    /// packet passes as it is.
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
        let Some(state) = self
            .channels
            .get_mut(usize::from(group))
            .and_then(|group| group.get_mut(usize::from(channel)))
        else {
            return;
        };
        state.translate(message, |message, origin| {
            let voice = ChannelVoice {
                group,
                channel,
                message,
            };
            out.push(Event {
                origin,
                ..voice.event(event.offset)
            });
        });
    }

    /// Sends, at `last_frame`, every high byte still held: the end of a
    /// block is the timeout after which a data entry with no low byte goes
    /// out on its own.
    ///
    /// Thread: audio. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn end_block(&mut self, last_frame: u32, out: &mut EventList) {
        for (group, channels) in (0_u8..).zip(self.channels.iter_mut()) {
            for (channel, state) in (0_u8..).zip(channels.iter_mut()) {
                if let Some(entry) = state.numbers.flush() {
                    let voice = ChannelVoice {
                        group,
                        channel,
                        message: entry.message(),
                    };
                    out.push(Event {
                        origin: entry.origin(),
                        ..voice.event(last_frame)
                    });
                }
            }
        }
    }
}

impl Entry {
    /// The MIDI 2.0 controller it becomes.
    fn message(self) -> ChannelMessage {
        let Number {
            registered,
            bank,
            index,
        } = self.number;
        let low = self.low.unwrap_or(0);
        let value = up_14_to_32(join_14(low, self.high));
        if registered {
            ChannelMessage::RegisteredController { bank, index, value }
        } else {
            ChannelMessage::AssignableController { bank, index, value }
        }
    }

    /// How it was sent: with its low byte, or as a high byte alone, which
    /// goes out as if its low byte were 0.
    fn origin(self) -> Origin {
        if self.low.is_some() {
            Origin::Midi1
        } else {
            Origin::Midi1HighByte
        }
    }
}
