//! Reads and writes MIDI 1.0 channel voice packets (message type 2), which an
//! exporter upgrades on the way in and downgrades on the way out.

use crate::head::{
    CHANNEL_PRESSURE, CONTROL_CHANGE, Head, NOTE_OFF, NOTE_ON, PITCH_BEND, POLY_PRESSURE,
    PROGRAM_CHANGE, join_14, split_14,
};
use crate::{Event, MessageType};

/// A MIDI 1.0 channel voice message, with its group and channel.
///
/// # Examples
///
/// ```
/// use hapto_ump::{Midi1Message, Midi1Voice};
///
/// // A MIDI 1.0 source sends a note-on like this.
/// let voice = Midi1Voice { group: 0, channel: 0, message: Midi1Message::NoteOn { note: 60, velocity: 127 } };
/// assert_eq!(voice.words(), [0x2090_3C7F, 0, 0, 0]);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Midi1Voice {
    /// The UMP group, 0 to 15.
    pub group: u8,
    /// The channel, 0 to 15.
    pub channel: u8,
    /// The message.
    pub message: Midi1Message,
}

/// A MIDI 1.0 channel voice message, with values at MIDI 1.0 widths.
///
/// # Examples
///
/// ```
/// use hapto_ump::{ChannelMessage, EventList, Midi1Message, Midi1Voice, Upgrade};
///
/// // MIDI 1.0 treats a note-on with velocity 0 as a note-off, so `Upgrade`
/// // sends a note-off.
/// let message = Midi1Message::NoteOn { note: 60, velocity: 0 };
/// let quiet = Midi1Voice { group: 0, channel: 0, message };
/// let mut out = EventList::with_capacity(4);
/// Upgrade::new().block(&[quiet.event(0)], &mut out);
/// let upgraded = out.as_slice()[0].channel_voice().map(|voice| voice.message);
/// assert!(matches!(upgraded, Some(ChannelMessage::NoteOff { note: 60, .. })));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    /// The pressure on one note.
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
    /// Reads a MIDI 1.0 channel voice packet, or returns `None` for another
    /// message type.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_ump::{Midi1Message, Midi1Voice};
    ///
    /// // The low 7 bits of a pitch bend come first.
    /// let voice = Midi1Voice::decode([0x20E0_0040, 0, 0, 0]);
    /// assert_eq!(voice.map(|voice| voice.message), Some(Midi1Message::PitchBend { value: 0x2000 }));
    /// ```
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn decode(words: [u32; 4]) -> Option<Self> {
        let [first, ..] = words;
        let head = Head::read(first);
        if head.kind != MessageType::MIDI1 {
            return None;
        }
        let (byte3, byte4) = (head.byte3 & 0x7F, head.byte4 & 0x7F);
        let message = match head.voice_status() {
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
                value: join_14(byte3, byte4),
            },
            _ => return None,
        };
        Some(Midi1Voice {
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
    /// use hapto_ump::{Midi1Message, Midi1Voice};
    ///
    /// // A value wider than 7 bits is masked, so 200 becomes 72.
    /// let voice = Midi1Voice { group: 0, channel: 0, message: Midi1Message::ControlChange { controller: 7, value: 200 } };
    /// assert_eq!(voice.words(), [0x20B0_0748, 0, 0, 0]);
    /// ```
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn words(&self) -> [u32; 4] {
        let (status, byte3, byte4) = match self.message {
            Midi1Message::NoteOff { note, velocity } => (NOTE_OFF, note, velocity),
            Midi1Message::NoteOn { note, velocity } => (NOTE_ON, note, velocity),
            Midi1Message::PolyPressure { note, value } => (POLY_PRESSURE, note, value),
            Midi1Message::ControlChange { controller, value } => {
                (CONTROL_CHANGE, controller, value)
            }
            Midi1Message::ProgramChange { program } => (PROGRAM_CHANGE, program, 0),
            Midi1Message::ChannelPressure { value } => (CHANNEL_PRESSURE, value, 0),
            Midi1Message::PitchBend { value } => {
                let (low, high) = split_14(value);
                (PITCH_BEND, low, high)
            }
        };
        let (group, channel) = (self.group, self.channel);
        let head = Head::voice(
            MessageType::MIDI1,
            group,
            status,
            channel,
            byte3 & 0x7F,
            byte4 & 0x7F,
        );
        [head.word(), 0, 0, 0]
    }

    /// Returns the packet as an event at `offset`, as a MIDI 1.0 source sends
    /// it before any upgrade.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn event(&self, offset: u32) -> Event {
        Event::new(offset, self.words())
    }
}
