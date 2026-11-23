//! MIDI 2.0 channel voice messages (message type 4), read from and written
//! to packets.

use crate::Event;

/// MIDI 2.0 channel voice messages, two words.
const MIDI2: u32 = 0x4;

const REGISTERED_PER_NOTE: u32 = 0x0;
const ASSIGNABLE_PER_NOTE: u32 = 0x1;
const REGISTERED_CONTROLLER: u32 = 0x2;
const ASSIGNABLE_CONTROLLER: u32 = 0x3;
const RELATIVE_REGISTERED: u32 = 0x4;
const RELATIVE_ASSIGNABLE: u32 = 0x5;
const PER_NOTE_PITCH_BEND: u32 = 0x6;
const NOTE_OFF: u32 = 0x8;
const NOTE_ON: u32 = 0x9;
const POLY_PRESSURE: u32 = 0xA;
const CONTROL_CHANGE: u32 = 0xB;
const PROGRAM_CHANGE: u32 = 0xC;
const CHANNEL_PRESSURE: u32 = 0xD;
const PITCH_BEND: u32 = 0xE;
const PER_NOTE_MANAGEMENT: u32 = 0xF;

/// A MIDI 2.0 channel voice message, with the group and channel it came on.
///
/// # Examples
///
/// ```
/// use hapto_ump::{Attribute, ChannelMessage, ChannelVoice};
///
/// // Group 2, channel 5: a note-on at full velocity.
/// let voice = ChannelVoice {
///     group: 2,
///     channel: 5,
///     message: ChannelMessage::NoteOn { note: 64, velocity: u16::MAX, attribute: Attribute::NONE },
/// };
/// assert_eq!(voice.words(), [0x4295_4000, 0xFFFF_0000, 0, 0]);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelVoice {
    /// The UMP group, 0 to 15.
    pub group: u8,
    /// The channel, 0 to 15.
    pub channel: u8,
    /// What it says.
    pub message: ChannelMessage,
}

/// The MIDI 2.0 channel voice messages, each value at the width MIDI 2.0
/// sends it: nothing here rescales. Everything above the exporters reads
/// MIDI in this form, since an exporter upgrades MIDI 1.0 on the way in
/// (see [`Upgrade`](crate::Upgrade)).
///
/// A note, controller, bank or index field is 7 bits, but a per-note
/// controller's index, which is 8.
///
/// # Examples
///
/// ```
/// use hapto_ump::{Attribute, ChannelMessage};
///
/// // A velocity of 0 is a note-on in MIDI 2.0, unlike MIDI 1.0.
/// fn sounds(message: ChannelMessage) -> bool {
///     matches!(message, ChannelMessage::NoteOn { .. })
/// }
/// assert!(sounds(ChannelMessage::NoteOn { note: 60, velocity: 0, attribute: Attribute::NONE }));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// A note starts. A velocity of 0 is a note-on too.
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
    /// A registered controller: MIDI 1.0's RPN as one message.
    RegisteredController {
        /// The bank, 0 to 127.
        bank: u8,
        /// The index in the bank, 0 to 127.
        index: u8,
        /// Its value, 0 to `u32::MAX`.
        value: u32,
    },
    /// An assignable controller: MIDI 1.0's NRPN as one message.
    AssignableController {
        /// The bank, 0 to 127.
        bank: u8,
        /// The index in the bank, 0 to 127.
        index: u8,
        /// Its value, 0 to `u32::MAX`.
        value: u32,
    },
    /// A change to a registered controller's value, relative to where it is.
    RelativeRegisteredController {
        /// The bank, 0 to 127.
        bank: u8,
        /// The index in the bank, 0 to 127.
        index: u8,
        /// The change.
        change: i32,
    },
    /// A change to an assignable controller's value, relative to where it
    /// is.
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
        /// Notes already sounding on it stop taking per-note controllers.
        detach: bool,
        /// Its per-note controllers go back to their defaults.
        reset: bool,
    },
}

/// A note-on's or note-off's attribute: what its type says the data
/// means. Type 0 is none.
///
/// # Examples
///
/// ```
/// use hapto_ump::Attribute;
///
/// // Type 3 is pitch 7.9: here a quarter tone above A4.
/// let pitch = Attribute { kind: 3, data: 69 << 9 | 0x80 };
/// assert_ne!(pitch, Attribute::NONE);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Attribute {
    /// The attribute type, 0 for none.
    pub kind: u8,
    /// The attribute data, as its type reads it.
    pub data: u16,
}

impl Attribute {
    /// No attribute.
    pub const NONE: Attribute = Attribute { kind: 0, data: 0 };
}

/// The bank a program change selects first.
///
/// # Examples
///
/// ```
/// use hapto_ump::{Bank, ChannelMessage, ChannelVoice};
///
/// // Program 5 of bank 1, 0: MIDI 1.0's controllers 0 and 32, then the program.
/// let message = ChannelMessage::ProgramChange { program: 5, bank: Some(Bank { msb: 1, lsb: 0 }) };
/// let voice = ChannelVoice { group: 0, channel: 0, message };
/// assert_eq!(voice.words(), [0x40C0_0001, 0x0500_0100, 0, 0]);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Bank {
    /// Bank select's most significant 7 bits: MIDI 1.0's controller 0.
    pub msb: u8,
    /// Its least significant 7 bits: MIDI 1.0's controller 32.
    pub lsb: u8,
}

impl ChannelVoice {
    /// Reads a packet. `None` for any other message type.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_ump::{ChannelMessage, ChannelVoice};
    ///
    /// // A registered controller: bank 0, index 2, half of full scale.
    /// let voice = ChannelVoice::decode([0x4020_0002, 0x8000_0000, 0, 0]);
    /// assert_eq!(
    ///     voice.map(|voice| voice.message),
    ///     Some(ChannelMessage::RegisteredController { bank: 0, index: 2, value: 0x8000_0000 })
    /// );
    ///
    /// // A MIDI 1.0 packet is not one: an exporter upgrades it first.
    /// assert_eq!(ChannelVoice::decode([0x2090_3C7F, 0, 0, 0]), None);
    /// ```
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn decode(words: [u32; 4]) -> Option<Self> {
        let [head, data, ..] = words;
        if head >> 28 != MIDI2 {
            return None;
        }
        let status = (head >> 20) & 0xF;
        let byte3 = ((head >> 8) & 0xFF) as u8;
        let byte4 = (head & 0xFF) as u8;
        let (seven3, seven4) = (byte3 & 0x7F, byte4 & 0x7F);
        let attribute = Attribute {
            kind: byte4,
            data: (data & 0xFFFF) as u16,
        };
        let velocity = (data >> 16) as u16;
        let message = match status {
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
            group: ((head >> 24) & 0xF) as u8,
            channel: ((head >> 16) & 0xF) as u8,
            message,
        })
    }

    /// The packet, with every field masked to its width.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_ump::{ChannelMessage, ChannelVoice};
    ///
    /// // A controller number past its 7 bits is masked: 200 becomes 72.
    /// let voice = ChannelVoice {
    ///     group: 0,
    ///     channel: 0,
    ///     message: ChannelMessage::ControlChange { controller: 200, value: 1 },
    /// };
    /// assert_eq!(voice.words(), [0x40B0_4800, 1, 0, 0]);
    /// ```
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn words(&self) -> [u32; 4] {
        let seven = |byte: u8| u32::from(byte & 0x7F);
        let head = |status: u32, byte3: u32, byte4: u32| {
            MIDI2 << 28
                | u32::from(self.group & 0xF) << 24
                | status << 20
                | u32::from(self.channel & 0xF) << 16
                | byte3 << 8
                | byte4
        };
        let note = |status: u32, note: u8, velocity: u16, attribute: Attribute| {
            (
                head(status, seven(note), u32::from(attribute.kind)),
                u32::from(velocity) << 16 | u32::from(attribute.data),
            )
        };
        let controller = |status: u32, bank: u8, index: u8| head(status, seven(bank), seven(index));
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
                    Some(Bank { msb, lsb }) => (1, seven(msb) << 8 | seven(lsb)),
                    None => (0, 0),
                };
                (head(PROGRAM_CHANGE, 0, valid), seven(program) << 24 | banks)
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
            ChannelMessage::RegisteredPerNoteController { note, index, value } => (
                head(REGISTERED_PER_NOTE, seven(note), u32::from(index)),
                value,
            ),
            ChannelMessage::AssignablePerNoteController { note, index, value } => (
                head(ASSIGNABLE_PER_NOTE, seven(note), u32::from(index)),
                value,
            ),
            ChannelMessage::PerNotePitchBend { note, value } => {
                (head(PER_NOTE_PITCH_BEND, seven(note), 0), value)
            }
            ChannelMessage::PerNoteManagement {
                note,
                detach,
                reset,
            } => {
                let flags = u32::from(detach) << 1 | u32::from(reset);
                (head(PER_NOTE_MANAGEMENT, seven(note), flags), 0)
            }
        };
        [head, data, 0, 0]
    }

    /// The packet as an event at `offset`, sent as it is, ready to push on
    /// an [`EventList`](crate::EventList).
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn event(&self, offset: u32) -> Event {
        Event::new(offset, self.words())
    }
}
