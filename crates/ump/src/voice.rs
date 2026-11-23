//! Reads and writes MIDI 2.0 channel voice messages (message type 4) in
//! packets.

use crate::head::{
    CHANNEL_PRESSURE, CONTROL_CHANGE, Head, NOTE_OFF, NOTE_ON, PITCH_BEND, POLY_PRESSURE,
    PROGRAM_CHANGE,
};
use crate::{Event, MessageType};

// These statuses exist only in MIDI 2.0.
const REGISTERED_PER_NOTE: u8 = 0x0;
const ASSIGNABLE_PER_NOTE: u8 = 0x1;
const REGISTERED_CONTROLLER: u8 = 0x2;
const ASSIGNABLE_CONTROLLER: u8 = 0x3;
const RELATIVE_REGISTERED: u8 = 0x4;
const RELATIVE_ASSIGNABLE: u8 = 0x5;
const PER_NOTE_PITCH_BEND: u8 = 0x6;
const PER_NOTE_MANAGEMENT: u8 = 0xF;

/// A MIDI 2.0 channel voice message, with the group and channel that it came
/// on.
///
/// # Examples
///
/// ```
/// use hapto_ump::{Attribute, ChannelMessage, ChannelVoice};
///
/// // A note-on at full velocity on group 2, channel 5.
/// let voice = ChannelVoice {
///     group: 2,
///     channel: 5,
///     message: ChannelMessage::NoteOn { note: 64, velocity: u16::MAX, attribute: Attribute::NONE },
/// };
/// assert_eq!(voice.words(), [0x4295_4000, 0xFFFF_0000, 0, 0]);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChannelVoice {
    /// The UMP group, 0 to 15.
    pub group: u8,
    /// The channel, 0 to 15.
    pub channel: u8,
    /// The message.
    pub message: ChannelMessage,
}

/// A MIDI 2.0 channel voice message, with each value at its MIDI 2.0 width.
///
/// Nothing here rescales a value. Everything above the exporters reads this
/// form, as the [crate](crate) docs describe.
///
/// Note, controller, bank and index fields are 7 bits, except a per-note
/// controller's index, which is 8.
///
/// # Examples
///
/// ```
/// use hapto_ump::{Attribute, ChannelMessage};
///
/// // Unlike in MIDI 1.0, a note-on at velocity 0 is still a note-on.
/// fn sounds(message: ChannelMessage) -> bool {
///     matches!(message, ChannelMessage::NoteOn { .. })
/// }
/// assert!(sounds(ChannelMessage::NoteOn { note: 60, velocity: 0, attribute: Attribute::NONE }));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ChannelMessage {
    /// A note ends.
    NoteOff {
        /// The note, 0 to 127.
        note: u8,
        /// Its release velocity, 0 to 65535.
        velocity: u16,
        /// Its attribute, if any.
        attribute: Attribute,
    },
    /// A note starts, even at velocity 0.
    NoteOn {
        /// The note, 0 to 127.
        note: u8,
        /// Its velocity, 0 to 65535.
        velocity: u16,
        /// Its attribute, if any.
        attribute: Attribute,
    },
    /// One note's pressure.
    PolyPressure {
        /// The note, 0 to 127.
        note: u8,
        /// The pressure, 0 to `u32::MAX`.
        value: u32,
    },
    /// A control change.
    ControlChange {
        /// The controller, 0 to 127.
        controller: u8,
        /// Its value, 0 to `u32::MAX`.
        value: u32,
    },
    /// A program change, with the bank to select first when there is one.
    ProgramChange {
        /// The program, 0 to 127.
        program: u8,
        /// The bank, when the sender selects one with the program.
        bank: Option<Bank>,
    },
    /// The channel's pressure.
    ChannelPressure {
        /// The pressure, 0 to `u32::MAX`.
        value: u32,
    },
    /// The channel's pitch bend, centred at `0x8000_0000`.
    PitchBend {
        /// The bend, 0 to `u32::MAX`.
        value: u32,
    },
    /// A registered controller, which is MIDI 1.0's RPN as one message.
    RegisteredController {
        /// The bank, 0 to 127.
        bank: u8,
        /// The index in the bank, 0 to 127.
        index: u8,
        /// Its value, 0 to `u32::MAX`.
        value: u32,
    },
    /// An assignable controller, which is MIDI 1.0's NRPN as one message.
    AssignableController {
        /// The bank, 0 to 127.
        bank: u8,
        /// The index in the bank, 0 to 127.
        index: u8,
        /// Its value, 0 to `u32::MAX`.
        value: u32,
    },
    /// A relative change to a registered controller's value.
    RelativeRegisteredController {
        /// The bank, 0 to 127.
        bank: u8,
        /// The index in the bank, 0 to 127.
        index: u8,
        /// The change.
        change: i32,
    },
    /// A relative change to an assignable controller's value.
    RelativeAssignableController {
        /// The bank, 0 to 127.
        bank: u8,
        /// The index in the bank, 0 to 127.
        index: u8,
        /// The change.
        change: i32,
    },
    /// One note's registered controller.
    RegisteredPerNoteController {
        /// The note, 0 to 127.
        note: u8,
        /// The controller, 0 to 255.
        index: u8,
        /// Its value, 0 to `u32::MAX`.
        value: u32,
    },
    /// One note's assignable controller.
    AssignablePerNoteController {
        /// The note, 0 to 127.
        note: u8,
        /// The controller, 0 to 255.
        index: u8,
        /// Its value, 0 to `u32::MAX`.
        value: u32,
    },
    /// One note's pitch bend, centred at `0x8000_0000`.
    PerNotePitchBend {
        /// The note, 0 to 127.
        note: u8,
        /// The bend, 0 to `u32::MAX`.
        value: u32,
    },
    /// What happens to the per-note controllers of the notes on a key.
    PerNoteManagement {
        /// The note, 0 to 127.
        note: u8,
        /// Whether notes already sounding on the key stop receiving per-note
        /// controllers.
        detach: bool,
        /// Whether the key's per-note controllers reset to their defaults.
        reset: bool,
    },
}

/// A note-on's or note-off's attribute, whose type says what the data means.
///
/// Type 0 means no attribute.
///
/// # Examples
///
/// ```
/// use hapto_ump::Attribute;
///
/// // Type 3 is pitch 7.9, and this data is a quarter tone above A4.
/// let pitch = Attribute { kind: 3, data: 69 << 9 | 0x100 };
/// assert_ne!(pitch, Attribute::NONE);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Attribute {
    /// The attribute type, 0 for none.
    pub kind: u8,
    /// The attribute data, whose meaning depends on the type.
    pub data: u16,
}

impl Attribute {
    /// No attribute.
    pub const NONE: Attribute = Attribute { kind: 0, data: 0 };

    /// The type of a note's pitch: 7.9 fixed point, 7 bits of key and 9 of
    /// fraction.
    pub const PITCH_7_9: u8 = 3;

    /// A pitch attribute of `semitones`, key 0 at 0, held to 0 to just under
    /// 128.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_ump::Attribute;
    ///
    /// let pitch = Attribute::pitch(69.5);
    /// assert_eq!(pitch, Attribute { kind: Attribute::PITCH_7_9, data: 69 << 9 | 0x100 });
    /// assert_eq!(pitch.semitones(), Some(69.5));
    /// assert_eq!(Attribute::NONE.semitones(), None);
    /// ```
    pub fn pitch(semitones: f64) -> Attribute {
        Attribute {
            kind: Attribute::PITCH_7_9,
            data: (semitones.clamp(0.0, 127.998) * 512.0).round() as u16,
        }
    }

    /// The pitch a pitch attribute names, in semitones; `None` for an
    /// attribute of any other type.
    pub fn semitones(self) -> Option<f64> {
        (self.kind == Attribute::PITCH_7_9).then(|| f64::from(self.data) / 512.0)
    }
}

/// The bank that a program change selects first.
///
/// # Examples
///
/// ```
/// use hapto_ump::{Bank, ChannelMessage, ChannelVoice};
///
/// // Program 5 of bank 1, 0, which MIDI 1.0 sends as controllers 0 and 32,
/// // then the program.
/// let message = ChannelMessage::ProgramChange { program: 5, bank: Some(Bank { msb: 1, lsb: 0 }) };
/// let voice = ChannelVoice { group: 0, channel: 0, message };
/// assert_eq!(voice.words(), [0x40C0_0001, 0x0500_0100, 0, 0]);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Bank {
    /// Bank select's most significant 7 bits, sent as controller 0 in MIDI
    /// 1.0.
    pub msb: u8,
    /// Its least significant 7 bits, sent as controller 32 in MIDI 1.0.
    pub lsb: u8,
}

impl ChannelMessage {
    /// The registered per-note controller of a note's absolute pitch, 7.25
    /// fixed point, as [`scale::pitch_7_25`](crate::scale::pitch_7_25)
    /// writes it.
    pub const PER_NOTE_PITCH: u8 = 3;
}

impl ChannelVoice {
    /// Reads a channel voice message from a packet, or returns `None` for any
    /// other message type.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_ump::{ChannelMessage, ChannelVoice};
    ///
    /// // A registered controller at bank 0, index 2, at half scale.
    /// let voice = ChannelVoice::decode([0x4020_0002, 0x8000_0000, 0, 0]);
    /// assert_eq!(
    ///     voice.map(|voice| voice.message),
    ///     Some(ChannelMessage::RegisteredController { bank: 0, index: 2, value: 0x8000_0000 })
    /// );
    ///
    /// // A MIDI 1.0 packet returns `None`, so an exporter must upgrade it
    /// // first.
    /// assert_eq!(ChannelVoice::decode([0x2090_3C7F, 0, 0, 0]), None);
    /// ```
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn decode(words: [u32; 4]) -> Option<Self> {
        let [first, data, ..] = words;
        let head = Head::read(first);
        if head.kind != MessageType::MIDI2 {
            return None;
        }
        let (byte3, byte4) = (head.byte3, head.byte4);
        let (seven3, seven4) = (byte3 & 0x7F, byte4 & 0x7F);
        let attribute = Attribute {
            kind: byte4,
            data: (data & 0xFFFF) as u16,
        };
        let velocity = (data >> 16) as u16;
        let message = match head.voice_status() {
            NOTE_OFF => ChannelMessage::NoteOff {
                note: seven3,
                velocity,
                attribute,
            },
            NOTE_ON => ChannelMessage::NoteOn {
                note: seven3,
                velocity,
                attribute,
            },
            POLY_PRESSURE => ChannelMessage::PolyPressure {
                note: seven3,
                value: data,
            },
            CONTROL_CHANGE => ChannelMessage::ControlChange {
                controller: seven3,
                value: data,
            },
            PROGRAM_CHANGE => ChannelMessage::ProgramChange {
                program: ((data >> 24) & 0x7F) as u8,
                bank: (byte4 & 0x01 != 0).then_some(Bank {
                    msb: ((data >> 8) & 0x7F) as u8,
                    lsb: (data & 0x7F) as u8,
                }),
            },
            CHANNEL_PRESSURE => ChannelMessage::ChannelPressure { value: data },
            PITCH_BEND => ChannelMessage::PitchBend { value: data },
            REGISTERED_CONTROLLER => ChannelMessage::RegisteredController {
                bank: seven3,
                index: seven4,
                value: data,
            },
            ASSIGNABLE_CONTROLLER => ChannelMessage::AssignableController {
                bank: seven3,
                index: seven4,
                value: data,
            },
            RELATIVE_REGISTERED => ChannelMessage::RelativeRegisteredController {
                bank: seven3,
                index: seven4,
                change: data.cast_signed(),
            },
            RELATIVE_ASSIGNABLE => ChannelMessage::RelativeAssignableController {
                bank: seven3,
                index: seven4,
                change: data.cast_signed(),
            },
            REGISTERED_PER_NOTE => ChannelMessage::RegisteredPerNoteController {
                note: seven3,
                index: byte4,
                value: data,
            },
            ASSIGNABLE_PER_NOTE => ChannelMessage::AssignablePerNoteController {
                note: seven3,
                index: byte4,
                value: data,
            },
            PER_NOTE_PITCH_BEND => ChannelMessage::PerNotePitchBend {
                note: seven3,
                value: data,
            },
            PER_NOTE_MANAGEMENT => ChannelMessage::PerNoteManagement {
                note: seven3,
                detach: byte4 & 0x02 != 0,
                reset: byte4 & 0x01 != 0,
            },
            _ => return None,
        };
        Some(ChannelVoice {
            group: head.group,
            channel: head.channel(),
            message,
        })
    }

    /// Returns the packet, with every field masked to its width.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_ump::{ChannelMessage, ChannelVoice};
    ///
    /// // A controller number wider than 7 bits is masked, so 200 becomes 72.
    /// let voice = ChannelVoice {
    ///     group: 0,
    ///     channel: 0,
    ///     message: ChannelMessage::ControlChange { controller: 200, value: 1 },
    /// };
    /// assert_eq!(voice.words(), [0x40B0_4800, 1, 0, 0]);
    /// ```
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn words(&self) -> [u32; 4] {
        let seven = |byte: u8| byte & 0x7F;
        let head = |status: u8, byte3: u8, byte4: u8| {
            let (group, channel) = (self.group, self.channel);
            Head::voice(MessageType::MIDI2, group, status, channel, byte3, byte4).word()
        };
        let note = |status: u8, note: u8, velocity: u16, attribute: Attribute| {
            (
                head(status, seven(note), attribute.kind),
                u32::from(velocity) << 16 | u32::from(attribute.data),
            )
        };
        let controller = |status: u8, bank: u8, index: u8| head(status, seven(bank), seven(index));
        let (head, data) = match self.message {
            ChannelMessage::NoteOff {
                note: key,
                velocity,
                attribute,
            } => note(NOTE_OFF, key, velocity, attribute),
            ChannelMessage::NoteOn {
                note: key,
                velocity,
                attribute,
            } => note(NOTE_ON, key, velocity, attribute),
            ChannelMessage::PolyPressure { note, value } => {
                (head(POLY_PRESSURE, seven(note), 0), value)
            }
            ChannelMessage::ControlChange { controller, value } => {
                (head(CONTROL_CHANGE, seven(controller), 0), value)
            }
            ChannelMessage::ProgramChange { program, bank } => {
                let (valid, banks) = match bank {
                    Some(Bank { msb, lsb }) => {
                        (1, u32::from(seven(msb)) << 8 | u32::from(seven(lsb)))
                    }
                    None => (0, 0),
                };
                let data = u32::from(seven(program)) << 24 | banks;
                (head(PROGRAM_CHANGE, 0, valid), data)
            }
            ChannelMessage::ChannelPressure { value } => (head(CHANNEL_PRESSURE, 0, 0), value),
            ChannelMessage::PitchBend { value } => (head(PITCH_BEND, 0, 0), value),
            ChannelMessage::RegisteredController { bank, index, value } => {
                (controller(REGISTERED_CONTROLLER, bank, index), value)
            }
            ChannelMessage::AssignableController { bank, index, value } => {
                (controller(ASSIGNABLE_CONTROLLER, bank, index), value)
            }
            ChannelMessage::RelativeRegisteredController {
                bank,
                index,
                change,
            } => (
                controller(RELATIVE_REGISTERED, bank, index),
                change.cast_unsigned(),
            ),
            ChannelMessage::RelativeAssignableController {
                bank,
                index,
                change,
            } => (
                controller(RELATIVE_ASSIGNABLE, bank, index),
                change.cast_unsigned(),
            ),
            ChannelMessage::RegisteredPerNoteController { note, index, value } => {
                (head(REGISTERED_PER_NOTE, seven(note), index), value)
            }
            ChannelMessage::AssignablePerNoteController { note, index, value } => {
                (head(ASSIGNABLE_PER_NOTE, seven(note), index), value)
            }
            ChannelMessage::PerNotePitchBend { note, value } => {
                (head(PER_NOTE_PITCH_BEND, seven(note), 0), value)
            }
            ChannelMessage::PerNoteManagement {
                note,
                detach,
                reset,
            } => {
                let flags = u8::from(detach) << 1 | u8::from(reset);
                (head(PER_NOTE_MANAGEMENT, seven(note), flags), 0)
            }
        };
        [head, data, 0, 0]
    }

    /// Returns the packet as an event at `offset` with origin
    /// [`Origin::Midi2`](crate::Origin::Midi2), ready to push onto an
    /// [`EventList`](crate::EventList).
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn event(&self, offset: u32) -> Event {
        Event::new(offset, self.words())
    }
}
