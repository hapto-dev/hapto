//! Reads and writes a packet's first word byte by byte, and defines the
//! channel voice statuses that both protocols share.

use crate::MessageType;

pub(crate) const NOTE_OFF: u8 = 0x8;
pub(crate) const NOTE_ON: u8 = 0x9;
pub(crate) const POLY_PRESSURE: u8 = 0xA;
pub(crate) const CONTROL_CHANGE: u8 = 0xB;
pub(crate) const PROGRAM_CHANGE: u8 = 0xC;
pub(crate) const CHANNEL_PRESSURE: u8 = 0xD;
pub(crate) const PITCH_BEND: u8 = 0xE;

/// A packet's first word, split into its message type, group, status byte and
/// the two bytes after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Head {
    pub(crate) kind: MessageType,
    pub(crate) group: u8,
    /// The status byte. For a channel voice message, the high four bits are
    /// the status and the low four are the channel.
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

    /// Builds a channel voice message's first word, packing the four-bit
    /// `status` and `channel` into the status byte.
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

    /// Returns a channel voice message's status, the status byte's high four
    /// bits.
    pub(crate) const fn voice_status(self) -> u8 {
        self.status >> 4
    }

    /// Returns a channel voice message's channel, the status byte's low four
    /// bits.
    pub(crate) const fn channel(self) -> u8 {
        self.status & 0xF
    }
}

/// Splits a 14-bit value into its low and high 7 bits, in the order that MIDI
/// 1.0 sends them.
pub(crate) const fn split_14(value: u16) -> (u8, u8) {
    ((value & 0x7F) as u8, ((value >> 7) & 0x7F) as u8)
}

/// Returns the 14-bit value with these low and high 7 bits.
pub(crate) const fn join_14(low: u8, high: u8) -> u16 {
    (high as u16 & 0x7F) << 7 | (low as u16 & 0x7F)
}
