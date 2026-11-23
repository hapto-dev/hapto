//! 7-bit System Exclusive packets: a SysEx message six bytes at a time.

use crate::buf::Buf;
use crate::{Event, MessageType};

/// Where a [`SysEx7`] packet's bytes fall in their message.
///
/// # Examples
///
/// ```
/// use hapto_ump::SysExForm;
///
/// assert!(SysExForm::Complete.begins() && SysExForm::Complete.ends());
/// assert!(SysExForm::Start.begins() && !SysExForm::Start.ends());
/// assert!(!SysExForm::Continue.begins() && !SysExForm::Continue.ends());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SysExForm {
    /// The whole message, in this one packet.
    Complete,
    /// The message's first packet, with more to come.
    Start,
    /// A packet from its middle.
    Continue,
    /// Its last packet.
    End,
}

impl SysExForm {
    /// Whether a packet of this form begins a message: a complete one or a
    /// start.
    pub const fn begins(self) -> bool {
        matches!(self, SysExForm::Complete | SysExForm::Start)
    }

    /// Whether a packet of this form ends a message: a complete one or an
    /// end.
    pub const fn ends(self) -> bool {
        matches!(self, SysExForm::Complete | SysExForm::End)
    }

    /// The form of packet `index`, from 0, of a message `count` packets
    /// long.
    const fn of(index: usize, count: usize) -> SysExForm {
        if count <= 1 {
            SysExForm::Complete
        } else if index == 0 {
            SysExForm::Start
        } else if index.saturating_add(1) >= count {
            SysExForm::End
        } else {
            SysExForm::Continue
        }
    }

    const fn status(self) -> u32 {
        match self {
            SysExForm::Complete => 0x0,
            SysExForm::Start => 0x1,
            SysExForm::Continue => 0x2,
            SysExForm::End => 0x3,
        }
    }

    /// The form a packet's status names; `None` for the reserved 4 to 15.
    const fn from_status(status: u32) -> Option<SysExForm> {
        match status {
            0x0 => Some(SysExForm::Complete),
            0x1 => Some(SysExForm::Start),
            0x2 => Some(SysExForm::Continue),
            0x3 => Some(SysExForm::End),
            _ => None,
        }
    }
}

/// One 7-bit System Exclusive packet, message type 3: up to six bytes of a
/// SysEx message, the group it is on, and where the bytes fall in the
/// message.
///
/// A message's bytes are its data alone, each of 7 bits, without the 0xF0
/// and 0xF7 that frame it in MIDI 1.0's byte stream. Both protocols carry
/// SysEx this way.
///
/// # Examples
///
/// ```
/// use hapto_ump::{SysEx7, SysExForm};
///
/// // An identity request, whole in one packet on group 0.
/// let packet = SysEx7::new(0, SysExForm::Complete, &[0x7E, 0x7F, 0x06, 0x01]);
/// assert_eq!(packet.words(), [0x3004_7E7F, 0x0601_0000, 0, 0]);
/// assert_eq!(SysEx7::decode(packet.words()), Some(packet));
/// assert_eq!(packet.data(), [0x7E, 0x7F, 0x06, 0x01]);
///
/// // Twenty bytes take four packets: a start, two continues and an end.
/// let mut forms = Vec::new();
/// SysEx7::packets(0, &[0x11; 20], |words| {
///     forms.push(SysEx7::decode(words).map(|packet| packet.form));
/// });
/// assert_eq!(forms.len(), SysEx7::count(20));
/// assert_eq!(forms[0], Some(SysExForm::Start));
/// assert_eq!(forms[3], Some(SysExForm::End));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SysEx7 {
    /// The group it is on, 0 to 15.
    pub group: u8,
    /// Where its bytes fall in their message.
    pub form: SysExForm,
    data: Buf<6>,
}

impl SysEx7 {
    /// The most bytes one packet carries.
    pub const MAX_DATA: usize = 6;

    /// A packet on `group`, 0 to 15, of form `form`, carrying the first six
    /// bytes of `data`, each held to 7 bits.
    pub fn new(group: u8, form: SysExForm, data: &[u8]) -> Self {
        let mut held = Buf::new();
        for &byte in data {
            if !held.push(byte & 0x7F) {
                break;
            }
        }
        SysEx7 {
            group: group & 0xF,
            form,
            data: held,
        }
    }

    /// The SysEx7 packet `words` carries. `None` for a packet of another
    /// message type, or one whose status is reserved. A count past six reads
    /// as six.
    ///
    /// Thread: any. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn decode(words: [u32; 4]) -> Option<Self> {
        let [head, second, ..] = words;
        if MessageType::of(head) != MessageType::SYSEX7 {
            return None;
        }
        let form = SysExForm::from_status((head >> 20) & 0xF)?;
        let count = (((head >> 16) & 0xF) as usize).min(Self::MAX_DATA);
        let [_, _, b1, b2] = head.to_be_bytes();
        let [b3, b4, b5, b6] = second.to_be_bytes();
        let bytes = [b1, b2, b3, b4, b5, b6];
        Some(SysEx7::new(
            ((head >> 24) & 0xF) as u8,
            form,
            bytes.get(..count).unwrap_or(&[]),
        ))
    }

    /// Its bytes, at most six.
    pub fn data(&self) -> &[u8] {
        self.data.as_slice()
    }

    /// It as a packet's words.
    pub fn words(&self) -> [u32; 4] {
        let mut bytes = [0_u8; 6];
        for (slot, &byte) in bytes.iter_mut().zip(self.data()) {
            *slot = byte;
        }
        let [b1, b2, b3, b4, b5, b6] = bytes;
        let count = self.data().len() as u8;
        let head = u32::from_be_bytes([
            (MessageType::SYSEX7.get() << 4) | (self.group & 0xF),
            ((self.form.status() as u8) << 4) | count,
            b1,
            b2,
        ]);
        [head, u32::from_be_bytes([b3, b4, b5, b6]), 0, 0]
    }

    /// It as an event at frame `offset`.
    pub fn event(&self, offset: u32) -> Event {
        Event::new(offset, self.words())
    }

    /// Splits a whole message's `data` into its packets on `group`, handing
    /// `send` each one's words in order: one complete packet for six bytes
    /// or fewer, none included, and otherwise a start, the continues and an
    /// end. Each byte is held to 7 bits.
    ///
    /// Thread: any. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn packets(group: u8, data: &[u8], mut send: impl FnMut([u32; 4])) {
        let count = Self::count(data.len());
        let mut chunks = data.chunks(Self::MAX_DATA);
        for index in 0..count {
            let chunk = chunks.next().unwrap_or(&[]);
            send(SysEx7::new(group, SysExForm::of(index, count), chunk).words());
        }
    }

    /// How many packets a message of `len` bytes takes: one for six bytes or
    /// fewer, none included.
    pub const fn count(len: usize) -> usize {
        let count = len.div_ceil(Self::MAX_DATA);
        if count == 0 { 1 } else { count }
    }
}

impl Event {
    /// The SysEx7 packet this event carries, as [`SysEx7::decode`] reads it.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn sysex7(&self) -> Option<SysEx7> {
        SysEx7::decode(self.words)
    }
}
