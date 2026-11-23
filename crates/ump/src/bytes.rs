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

use crate::buf::Buf;
use crate::{MessageType, SysEx7, SysExForm};

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
    /// data.
    status: Option<u8>,
    /// The data bytes that have come for it.
    data: Buf<2>,
    /// A SysEx under way, if one is.
    sysex: Option<SysEx>,
}

/// What a stream holds of a SysEx under way.
#[derive(Debug, Clone, Copy)]
struct SysEx {
    /// The bytes not yet sent, at most a packet's.
    held: Buf<{ SysEx7::MAX_DATA }>,
    /// Whether a packet of this SysEx went out already, so the next is not
    /// its first.
    begun: bool,
}

impl fmt::Debug for ByteStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ByteStream")
            .field("group", &self.group)
            .field("status", &format_args!("{:02X?}", self.status))
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
            status: None,
            data: Buf::new(),
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
        self.data.clear();
        self.status = None;
        if byte == SYSEX_BEGIN {
            self.sysex = Some(SysEx {
                held: Buf::new(),
                begun: false,
            });
            return;
        }
        match Bytes::length_of(byte) {
            // Tune request, which has no data.
            Some(1) => send(self.system(byte, 0, 0)),
            // A channel voice status, in force until another status, or a
            // system common one waiting for its data, which holds no
            // running status.
            Some(_) => self.status = Some(byte),
            // A stray end of SysEx, and 0xF4 and 0xF5, undefined, only end
            // the status in force.
            None => {}
        }
    }

    fn data_byte(&mut self, byte: u8, send: &mut impl FnMut([u32; 4])) {
        if let Some(sysex) = &mut self.sysex {
            if sysex.held.is_full() {
                // A seventh byte: the six held are not the last.
                let form = if sysex.begun {
                    SysExForm::Continue
                } else {
                    SysExForm::Start
                };
                send(SysEx7::new(self.group, form, sysex.held.as_slice()).words());
                sysex.begun = true;
                sysex.held.clear();
            }
            sysex.held.push(byte);
            return;
        }
        let Some(status) = self.status else {
            return;
        };
        self.data.push(byte);
        // A status this stream keeps has a length of 2 or 3, itself included.
        let wants = Bytes::length_of(status).map_or(1, |length| length.saturating_sub(1));
        if self.data.len() < wants {
            return;
        }
        let (first, second) = match *self.data.as_slice() {
            [first, second, ..] => (first, second),
            [first] => (first, 0),
            [] => (0, 0),
        };
        self.data.clear();
        if status >= 0xF0 {
            send(self.system(status, first, second));
            // A system common message holds no running status.
            self.status = None;
        } else {
            send(word(MessageType::MIDI1, self.group, status, first, second));
        }
    }

    /// Sends what `sysex` holds as its last packet: the whole of it if no
    /// packet of it went out yet.
    fn finish(&self, sysex: SysEx, send: &mut impl FnMut([u32; 4])) {
        let form = if sysex.begun {
            SysExForm::End
        } else {
            SysExForm::Complete
        };
        send(SysEx7::new(self.group, form, sysex.held.as_slice()).words());
    }

    fn system(&self, status: u8, first: u8, second: u8) -> [u32; 4] {
        word(MessageType::SYSTEM, self.group, status, first, second)
    }
}

/// A one-word packet of message type `kind`: a status byte and two data
/// bytes after the group.
fn word(kind: MessageType, group: u8, status: u8, first: u8, second: u8) -> [u32; 4] {
    let head = u32::from(kind.get()) << 28
        | u32::from(group & 0xF) << 24
        | u32::from(status) << 16
        | u32::from(first & 0x7F) << 8
        | u32::from(second & 0x7F);
    [head, 0, 0, 0]
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
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct Bytes(Buf<8>);

impl Bytes {
    /// `words` as MIDI 1.0 bytes. `None` for a packet MIDI 1.0's byte
    /// stream has no bytes for: a MIDI 2.0 message, which
    /// [`Downgrade`](crate::Downgrade) makes MIDI 1.0 first, a utility or
    /// stream message, 8-bit SysEx, or an undefined status.
    ///
    /// Thread: any. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn of(words: [u32; 4]) -> Option<Bytes> {
        let [head, ..] = words;
        let [_, status, ..] = head.to_be_bytes();
        match MessageType::of(head) {
            MessageType::SYSTEM if status >= 0xF0 => Bytes::short(head),
            MessageType::MIDI1 if status < 0xF0 => Bytes::short(head),
            MessageType::SYSEX7 => SysEx7::decode(words).map(Bytes::sysex),
            _ => None,
        }
    }

    /// How many bytes a MIDI 1.0 message with `status` takes, the status
    /// included: 3 for a note, a controller or a bend, 2 for a program
    /// change or channel pressure, 1 for a tune request or a real-time
    /// message. `None` for a data byte, for a SysEx's start or end, whose
    /// end decides its length, and for an undefined status.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_ump::Bytes;
    ///
    /// assert_eq!(Bytes::length_of(0x90), Some(3)); // a note-on
    /// assert_eq!(Bytes::length_of(0xC0), Some(2)); // a program change
    /// assert_eq!(Bytes::length_of(0xF8), Some(1)); // the clock
    /// assert_eq!(Bytes::length_of(0xF0), None); // a SysEx
    /// ```
    pub const fn length_of(status: u8) -> Option<usize> {
        match status {
            0x80..=0xBF | 0xE0..=0xEF | 0xF2 => Some(3),
            0xC0..=0xDF | 0xF1 | 0xF3 => Some(2),
            0xF6 | 0xF8 | 0xFA..=0xFC | 0xFE | 0xFF => Some(1),
            _ => None,
        }
    }

    /// A one-word packet's message: its status and the data bytes its
    /// length takes.
    fn short(head: u32) -> Option<Bytes> {
        let [_, status, first, second] = head.to_be_bytes();
        let length = Bytes::length_of(status)?;
        let mut out = Bytes::default();
        for byte in [status, first & 0x7F, second & 0x7F]
            .into_iter()
            .take(length)
        {
            out.0.push(byte);
        }
        Some(out)
    }

    /// A SysEx packet's part of its message, with 0xF0 where it begins one
    /// and 0xF7 where it ends one.
    fn sysex(packet: SysEx7) -> Bytes {
        let mut out = Bytes::default();
        if packet.form.begins() {
            out.0.push(SYSEX_BEGIN);
        }
        for &byte in packet.data() {
            out.0.push(byte);
        }
        if packet.form.ends() {
            out.0.push(SYSEX_FINISH);
        }
        out
    }
}

impl Deref for Bytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.0.as_slice()
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
