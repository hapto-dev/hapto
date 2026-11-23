//! A packet's first word byte by byte, and the statuses both protocols'
//! channel voice messages share.

use crate::MessageType;

pub(crate) const NOTE_OFF: u8 = 0x8;
pub(crate) const NOTE_ON: u8 = 0x9;
pub(crate) const POLY_PRESSURE: u8 = 0xA;
pub(crate) const CONTROL_CHANGE: u8 = 0xB;
pub(crate) const PROGRAM_CHANGE: u8 = 0xC;
pub(crate) const CHANNEL_PRESSURE: u8 = 0xD;
pub(crate) const PITCH_BEND: u8 = 0xE;

/// A packet's first word: its message type and group, then a status byte
/// and the two bytes after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Head {
    pub(crate) kind: MessageType,
    pub(crate) group: u8,
    /// The status byte. A channel voice message keeps its status in the
    /// high four bits and its channel in the low four.
    pub(crate) status: u8,
    pub(crate) byte3: u8,
    pub(crate) byte4: u8,
}

impl Head {
    pub(crate) const fn read(word: u32) -> Head {
        let [kind_group, status, byte3, byte4] = word.to_be_bytes();
        Head {
            kind: MessageType::of(word),
            group: kind_group & 0xF,
            status,
            byte3,
            byte4,
        }
    }

    /// A channel voice message's first word, its four-bit `status` and
    /// `channel` in the status byte.
    pub(crate) const fn voice(
        kind: MessageType,
        group: u8,
        status: u8,
        channel: u8,
        byte3: u8,
        byte4: u8,
    ) -> Head {
        Head {
            kind,
            group,
            status: (status & 0xF) << 4 | (channel & 0xF),
            byte3,
            byte4,
        }
    }

    pub(crate) const fn word(self) -> u32 {
        u32::from_be_bytes([
            self.kind.get() << 4 | (self.group & 0xF),
            self.status,
            self.byte3,
            self.byte4,
        ])
    }

    /// A channel voice message's status: the status byte's high four bits.
    pub(crate) const fn voice_status(self) -> u8 {
        self.status >> 4
    }

    /// A channel voice message's channel: the status byte's low four bits.
    pub(crate) const fn channel(self) -> u8 {
        self.status & 0xF
    }
}

/// A 14-bit value as MIDI 1.0 sends it: its low 7 bits, then its high 7.
pub(crate) const fn split_14(value: u16) -> (u8, u8) {
    ((value & 0x7F) as u8, ((value >> 7) & 0x7F) as u8)
}

/// The 14-bit value whose low and high 7 bits these are.
pub(crate) const fn join_14(low: u8, high: u8) -> u16 {
    (high as u16 & 0x7F) << 7 | (low as u16 & 0x7F)
}
