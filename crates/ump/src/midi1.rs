//! MIDI 1.0 channel voice messages (message type 2), read from and written
//! to packets: what an exporter upgrades on the way in and downgrades on
//! the way out.

use crate::Event;

/// MIDI 1.0 channel voice messages, one word.
const MIDI1: u32 = 0x2;

const NOTE_OFF: u32 = 0x8;
const NOTE_ON: u32 = 0x9;
const POLY_PRESSURE: u32 = 0xA;
const CONTROL_CHANGE: u32 = 0xB;
const PROGRAM_CHANGE: u32 = 0xC;
const CHANNEL_PRESSURE: u32 = 0xD;
const PITCH_BEND: u32 = 0xE;

/// A MIDI 1.0 channel voice message, with the group and channel it came on.
///
/// # Examples
///
/// ```
/// use hapto_ump::{Midi1Message, Midi1Voice};
///
/// // A MIDI 1.0 note-on, as a MIDI 1.0 source sends it.
/// let voice = Midi1Voice { group: 0, channel: 0, message: Midi1Message::NoteOn { note: 60, velocity: 127 } };
/// assert_eq!(voice.words(), [0x2090_3C7F, 0, 0, 0]);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Midi1Voice {
    /// The UMP group, 0 to 15.
    pub group: u8,
    /// The channel, 0 to 15.
    pub channel: u8,
    /// What it says.
    pub message: Midi1Message,
}

/// The MIDI 1.0 channel voice messages, each value at MIDI 1.0's own width.
///
/// # Examples
///
/// ```
/// use hapto_ump::Midi1Message;
///
/// // MIDI 1.0 reads a velocity of 0 as a note-off.
/// let quiet = Midi1Message::NoteOn { note: 60, velocity: 0 };
/// # let _ = quiet;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Midi1Message {
    /// A note ends.
    NoteOff {
        /// The note, 0 to 127.
        note: u8,
        /// Its release velocity, 0 to 127.
        velocity: u8,
    },
    /// A note starts, or ends when its velocity is 0.
    NoteOn {
        /// The note, 0 to 127.
        note: u8,
        /// Its velocity, 0 to 127.
        velocity: u8,
    },
    /// One note's pressure.
    PolyPressure {
        /// The note, 0 to 127.
        note: u8,
        /// The pressure, 0 to 127.
        value: u8,
    },
    /// A control change.
    ControlChange {
        /// The controller, 0 to 127.
        controller: u8,
        /// Its value, 0 to 127.
        value: u8,
    },
    /// A program change.
    ProgramChange {
        /// The program, 0 to 127.
        program: u8,
    },
    /// The channel's pressure.
    ChannelPressure {
        /// The pressure, 0 to 127.
        value: u8,
    },
    /// The channel's pitch bend, centred at `0x2000`.
    PitchBend {
        /// The bend, 0 to `0x3FFF`.
        value: u16,
    },
}

impl Midi1Voice {
    /// Reads a packet. `None` for any other message type.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_ump::{Midi1Message, Midi1Voice};
    ///
    /// // A pitch bend sends its low 7 bits first.
    /// let voice = Midi1Voice::decode([0x20E0_0040, 0, 0, 0]);
    /// assert_eq!(voice.map(|voice| voice.message), Some(Midi1Message::PitchBend { value: 0x2000 }));
    /// ```
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn decode(words: [u32; 4]) -> Option<Self> {
        let [head, ..] = words;
        if head >> 28 != MIDI1 {
            return None;
        }
        let byte3 = ((head >> 8) & 0x7F) as u8;
        let byte4 = (head & 0x7F) as u8;
        let message = match (head >> 20) & 0xF {
            NOTE_OFF => Midi1Message::NoteOff {
                note: byte3,
                velocity: byte4,
            },
            NOTE_ON => Midi1Message::NoteOn {
                note: byte3,
                velocity: byte4,
            },
            POLY_PRESSURE => Midi1Message::PolyPressure {
                note: byte3,
                value: byte4,
            },
            CONTROL_CHANGE => Midi1Message::ControlChange {
                controller: byte3,
                value: byte4,
            },
            PROGRAM_CHANGE => Midi1Message::ProgramChange { program: byte3 },
            CHANNEL_PRESSURE => Midi1Message::ChannelPressure { value: byte3 },
            PITCH_BEND => Midi1Message::PitchBend {
                value: u16::from(byte4) << 7 | u16::from(byte3),
            },
            _ => return None,
        };
        Some(Midi1Voice {
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
    /// use hapto_ump::{Midi1Message, Midi1Voice};
    ///
    /// // A value past its 7 bits is masked: 200 becomes 72.
    /// let voice = Midi1Voice { group: 0, channel: 0, message: Midi1Message::ControlChange { controller: 7, value: 200 } };
    /// assert_eq!(voice.words(), [0x20B0_0748, 0, 0, 0]);
    /// ```
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn words(&self) -> [u32; 4] {
        let seven = |byte: u8| u32::from(byte & 0x7F);
        let head = |status: u32, byte3: u32, byte4: u32| {
            MIDI1 << 28
                | u32::from(self.group & 0xF) << 24
                | status << 20
                | u32::from(self.channel & 0xF) << 16
                | byte3 << 8
                | byte4
        };
        let head = match self.message {
            Midi1Message::NoteOff { note, velocity } => {
                head(NOTE_OFF, seven(note), seven(velocity))
            }
            Midi1Message::NoteOn { note, velocity } => head(NOTE_ON, seven(note), seven(velocity)),
            Midi1Message::PolyPressure { note, value } => {
                head(POLY_PRESSURE, seven(note), seven(value))
            }
            Midi1Message::ControlChange { controller, value } => {
                head(CONTROL_CHANGE, seven(controller), seven(value))
            }
            Midi1Message::ProgramChange { program } => head(PROGRAM_CHANGE, seven(program), 0),
            Midi1Message::ChannelPressure { value } => head(CHANNEL_PRESSURE, seven(value), 0),
            Midi1Message::PitchBend { value } => head(
                PITCH_BEND,
                u32::from(value) & 0x7F,
                (u32::from(value) >> 7) & 0x7F,
            ),
        };
        [head, 0, 0, 0]
    }

    /// The packet as an event at `offset`, as a MIDI 1.0 source sends it,
    /// before any upgrade.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn event(&self, offset: u32) -> Event {
        Event::new(offset, self.words())
    }
}
