//! MIDI 1.0's byte stream, as a MIDI 1.0 device or port carries it, read
//! into packets and written back: the edge where a device's bytes become
//! UMP.

// Audio-path code: a panic here ends a host, so indexing, unwrap, expect,
// panic and unchecked arithmetic are refused, and a site that is provably
// safe says why in an `#[expect]`.
#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects
    )
)]

use std::fmt;
use std::ops::Deref;

/// System messages, one word.
const SYSTEM: u32 = 0x1;
/// MIDI 1.0 channel voice messages, one word.
const MIDI1: u32 = 0x2;
/// 7-bit System Exclusive, two words.
const SYSEX7: u32 = 0x3;

/// The SysEx7 statuses: the whole message in one packet, or its first,
/// middle and last packets.
const COMPLETE: u32 = 0x0;
const START: u32 = 0x1;
const CONTINUE: u32 = 0x2;
const END: u32 = 0x3;

/// The most data bytes one SysEx7 packet carries.
const SYSEX_BYTES: usize = 6;

const SYSEX_BEGIN: u8 = 0xF0;
const SYSEX_FINISH: u8 = 0xF7;

/// Reads a MIDI 1.0 byte stream into Universal MIDI Packets on one group:
/// channel voice messages as MIDI 1.0's (message type 2), system messages
/// as type 1, and System Exclusive as 7-bit SysEx packets (type 3).
///
/// It reads as MIDI 1.0 says. A channel voice status stays in force for the
/// data that follows it (running status), until another channel status or
/// a system common message. A real-time byte (0xF8 to 0xFF) goes out at
/// once wherever it falls, inside another message or a SysEx included, and
/// leaves both alone. A data byte with no status before it, an undefined
/// status and a stray end of SysEx are dropped. A status byte that cuts a
/// SysEx short ends it with what it holds.
///
/// A message may arrive across reads, as a device's buffers deliver a long
/// SysEx, so the stream keeps what it holds of one until the rest comes.
/// The packets go out as each message is whole, apart from a SysEx, which
/// goes out six bytes at a time as it arrives.
///
/// # Examples
///
/// ```
/// use hapto_ump::{ByteStream, Midi1Message, Midi1Voice};
///
/// // Two note-ons, the second under running status, with a clock between
/// // the first's bytes.
/// let mut stream = ByteStream::new(0);
/// let mut packets = Vec::new();
/// stream.read(&[0x90, 0x3C, 0xF8, 0x7F, 0x40, 0x7F], |words| packets.push(words));
///
/// assert_eq!(packets[0], [0x10F8_0000, 0, 0, 0], "the clock, at once");
/// let notes: Vec<_> = packets[1..].iter().filter_map(|&words| Midi1Voice::decode(words)).collect();
/// assert_eq!(notes[1].message, Midi1Message::NoteOn { note: 0x40, velocity: 0x7F });
/// ```
#[derive(Clone)]
pub struct ByteStream {
    group: u8,
    /// The status the next data bytes complete: a channel voice status, in
    /// force until another status, or a system common one waiting for its
    /// data. 0 for none.
    status: u8,
    /// The data bytes the status still wants.
    data: [u8; 2],
    /// How many of `data` have arrived.
    have: u8,
    /// A SysEx under way, if one is.
    sysex: Option<SysEx>,
}

/// What a stream holds of a SysEx under way.
#[derive(Debug, Clone, Copy)]
struct SysEx {
    /// The bytes not yet sent, at most a packet's.
    held: [u8; SYSEX_BYTES],
    /// How many of `held` there are.
    len: u8,
    /// Whether a packet of this SysEx went out already, so the next is not
    /// its first.
    begun: bool,
}

impl fmt::Debug for ByteStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ByteStream")
            .field("group", &self.group)
            .field("status", &format_args!("{:#04X}", self.status))
            .field("in_sysex", &self.sysex.is_some())
            .finish_non_exhaustive()
    }
}

impl ByteStream {
    /// A stream whose packets go out on `group`, 0 to 15, with nothing
    /// under way.
    pub const fn new(group: u8) -> Self {
        ByteStream {
            group: group & 0xF,
            status: 0,
            data: [0; 2],
            have: 0,
            sysex: None,
        }
    }

    /// Reads `bytes`, handing `send` each packet as it is whole.
    ///
    /// Thread: any. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn read(&mut self, bytes: &[u8], mut send: impl FnMut([u32; 4])) {
        for &byte in bytes {
            self.byte(byte, &mut send);
        }
    }

    /// Forgets a message under way, a SysEx included, and any running
    /// status, as when the device that sent them closes.
    pub fn reset(&mut self) {
        *self = ByteStream::new(self.group);
    }

    fn byte(&mut self, byte: u8, send: &mut impl FnMut([u32; 4])) {
        match byte {
            // Real time: at once, wherever it falls. 0xF9 and 0xFD are
            // undefined.
            0xF9 | 0xFD => {}
            0xF8..=0xFF => send(self.system(byte, 0, 0)),
            0x80..=0xF7 => self.status_byte(byte, send),
            _ => self.data_byte(byte, send),
        }
    }

    fn status_byte(&mut self, byte: u8, send: &mut impl FnMut([u32; 4])) {
        if let Some(sysex) = self.sysex.take() {
            self.finish(sysex, send);
            // The end of SysEx it was waiting for, or another status that
            // cut it short and is read on its own.
            if byte == SYSEX_FINISH {
                return;
            }
        }
        self.have = 0;
        self.status = 0;
        match byte {
            SYSEX_BEGIN => {
                self.sysex = Some(SysEx {
                    held: [0; SYSEX_BYTES],
                    len: 0,
                    begun: false,
                });
            }
            // Tune request, which has no data.
            0xF6 => send(self.system(byte, 0, 0)),
            // Time code quarter frame, song position and song select wait
            // for their data, and hold no running status.
            0xF1..=0xF3 => self.status = byte,
            // A channel voice status, in force until another status. A
            // stray end of SysEx and 0xF4 and 0xF5, undefined, only end it.
            0x80..=0xEF => self.status = byte,
            _ => {}
        }
    }

    fn data_byte(&mut self, byte: u8, send: &mut impl FnMut([u32; 4])) {
        if let Some(sysex) = &mut self.sysex {
            if usize::from(sysex.len) == SYSEX_BYTES {
                // A seventh byte: the six held are not the last.
                let status = if sysex.begun { CONTINUE } else { START };
                let packet = sysex7(self.group, status, &sysex.held);
                sysex.begun = true;
                sysex.len = 0;
                send(packet);
            }
            if let Some(slot) = sysex.held.get_mut(usize::from(sysex.len)) {
                *slot = byte;
                sysex.len = sysex.len.saturating_add(1);
            }
            return;
        }
        let wants = match self.status {
            0 => return,
            0xC0..=0xDF | 0xF1 | 0xF3 => 1,
            _ => 2,
        };
        if let Some(slot) = self.data.get_mut(usize::from(self.have)) {
            *slot = byte;
            self.have = self.have.saturating_add(1);
        }
        if self.have < wants {
            return;
        }
        self.have = 0;
        let [first, second] = self.data;
        let second = if wants == 2 { second } else { 0 };
        if self.status >= 0xF0 {
            send(self.system(self.status, first, second));
            // A system common message holds no running status.
            self.status = 0;
        } else {
            send(word(MIDI1, self.group, self.status, first, second));
        }
    }

    /// Sends what `sysex` holds as its last packet: the whole of it if no
    /// packet of it went out yet.
    fn finish(&self, sysex: SysEx, send: &mut impl FnMut([u32; 4])) {
        let status = if sysex.begun { END } else { COMPLETE };
        let held = sysex.held.get(..usize::from(sysex.len)).unwrap_or(&[]);
        send(sysex7(self.group, status, held));
    }

    fn system(&self, status: u8, first: u8, second: u8) -> [u32; 4] {
        word(SYSTEM, self.group, status, first, second)
    }
}

/// A one-word packet of message type `kind`: a status byte and two data
/// bytes after the group.
fn word(kind: u32, group: u8, status: u8, first: u8, second: u8) -> [u32; 4] {
    let head = kind << 28
        | u32::from(group & 0xF) << 24
        | u32::from(status) << 16
        | u32::from(first & 0x7F) << 8
        | u32::from(second & 0x7F);
    [head, 0, 0, 0]
}

/// A SysEx7 packet carrying `data`, at most six bytes.
fn sysex7(group: u8, status: u32, data: &[u8]) -> [u32; 4] {
    let mut bytes = [0_u8; SYSEX_BYTES];
    for (slot, &byte) in bytes.iter_mut().zip(data) {
        *slot = byte & 0x7F;
    }
    let count = data.len().min(SYSEX_BYTES) as u32;
    let [b1, b2, b3, b4, b5, b6] = bytes.map(u32::from);
    [
        SYSEX7 << 28 | u32::from(group & 0xF) << 24 | status << 20 | count << 16 | b1 << 8 | b2,
        b3 << 24 | b4 << 16 | b5 << 8 | b6,
        0,
        0,
    ]
}

/// One packet as MIDI 1.0 bytes, for a device or port that takes the byte
/// stream: at most eight, a short SysEx's start and end included.
///
/// [`of`](Self::of) writes MIDI 1.0's channel voice messages, system
/// messages and 7-bit SysEx. A SysEx packet writes its part of the message:
/// the first begins with 0xF0, the last ends with 0xF7, and a whole one in
/// one packet has both. Each is written in full, with no running status.
///
/// # Examples
///
/// ```
/// use hapto_ump::Bytes;
///
/// // A MIDI 1.0 note-on.
/// let bytes = Bytes::of([0x2090_3C7F, 0, 0, 0]);
/// assert_eq!(bytes.as_deref(), Some(&[0x90, 0x3C, 0x7F][..]));
///
/// // A MIDI 2.0 note-on is not MIDI 1.0: downgrade it first.
/// assert_eq!(Bytes::of([0x4090_3C00, 0xFFFF_0000, 0, 0]), None);
/// ```
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Bytes {
    bytes: [u8; 8],
    len: u8,
}

impl Bytes {
    /// `words` as MIDI 1.0 bytes. `None` for a packet MIDI 1.0's byte
    /// stream has no bytes for: a MIDI 2.0 message, which
    /// [`Downgrade`](crate::Downgrade) makes MIDI 1.0 first, a utility or
    /// stream message, 8-bit SysEx, or an undefined status.
    ///
    /// Thread: any. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn of(words: [u32; 4]) -> Option<Bytes> {
        let [head, second, ..] = words;
        let byte = |shift: u32| ((head >> shift) & 0xFF) as u8;
        let data = |shift: u32| ((head >> shift) & 0x7F) as u8;
        match head >> 28 {
            SYSTEM => {
                let status = byte(16);
                let length = match status {
                    0xF2 => 3,
                    0xF1 | 0xF3 => 2,
                    0xF6 | 0xF8 | 0xFA..=0xFC | 0xFE | 0xFF => 1,
                    _ => return None,
                };
                Some(Bytes::from_slice(&[status, data(8), data(0)], length))
            }
            MIDI1 => {
                let status = byte(16);
                let length = match status >> 4 {
                    0xC | 0xD => 2,
                    0x8..=0xE => 3,
                    _ => return None,
                };
                Some(Bytes::from_slice(&[status, data(8), data(0)], length))
            }
            SYSEX7 => {
                let count = usize::from(byte(16) & 0xF).min(SYSEX_BYTES);
                let payload = [
                    data(8),
                    data(0),
                    ((second >> 24) & 0x7F) as u8,
                    ((second >> 16) & 0x7F) as u8,
                    ((second >> 8) & 0x7F) as u8,
                    (second & 0x7F) as u8,
                ];
                let payload = payload.get(..count).unwrap_or(&[]);
                let (begins, ends) = match (head >> 20) & 0xF {
                    COMPLETE => (true, true),
                    START => (true, false),
                    CONTINUE => (false, false),
                    END => (false, true),
                    _ => return None,
                };
                let mut out = Bytes {
                    bytes: [0; 8],
                    len: 0,
                };
                if begins {
                    out.push(SYSEX_BEGIN);
                }
                for &byte in payload {
                    out.push(byte);
                }
                if ends {
                    out.push(SYSEX_FINISH);
                }
                Some(out)
            }
            _ => None,
        }
    }

    fn from_slice(bytes: &[u8; 3], length: usize) -> Bytes {
        let mut out = Bytes {
            bytes: [0; 8],
            len: 0,
        };
        for &byte in bytes.iter().take(length) {
            out.push(byte);
        }
        out
    }

    fn push(&mut self, byte: u8) {
        if let Some(slot) = self.bytes.get_mut(usize::from(self.len)) {
            *slot = byte;
            self.len = self.len.saturating_add(1);
        }
    }
}

impl Deref for Bytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.bytes.get(..usize::from(self.len)).unwrap_or(&[])
    }
}

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bytes({:02X?})", &**self)
    }
}
