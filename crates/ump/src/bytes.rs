//! Reads a MIDI 1.0 byte stream from a device or port into UMP packets, and
//! writes packets back as bytes.

use std::fmt;
use std::ops::Deref;

use crate::buf::Buf;
use crate::head::Head;
use crate::sysex::{SYSEX_BEGIN, SYSEX_FINISH};
use crate::{MessageType, SysEx7, SysExForm};

/// Reads a MIDI 1.0 byte stream into Universal MIDI Packets on one group:
/// channel voice messages as MIDI 1.0's (message type 2), system messages
/// as type 1, and System Exclusive as 7-bit SysEx packets (type 3).
///
/// Channel voice messages become message type 2, system messages type 1,
/// and System Exclusive 7-bit SysEx (type 3).
///
/// It reads by MIDI 1.0's rules:
///
/// - A channel voice status stays in force for the data bytes after it
///   (running status), until another channel status or a system common
///   message.
/// - A real-time byte (0xF8 to 0xFF) is sent at once, even inside another
///   message or a SysEx, and doesn't affect either.
/// - A data byte with no status before it, an undefined status and a stray
///   end of SysEx are dropped.
/// - A status byte inside a SysEx ends the SysEx with the bytes read so far.
///
/// A message can span several reads, as when a device delivers a long SysEx
/// in parts, so the stream keeps the partial message until the rest
/// arrives. Each message is sent once it is complete, except a SysEx, which
/// is sent six bytes at a time as it arrives.
///
/// # Examples
///
/// ```
/// use hapto_ump::{ByteStream, Midi1Message, Midi1Voice};
///
/// // The bytes are two note-ons, the second with running status, and a clock
/// // inside the first.
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
    /// The status that the next data bytes complete, either a running
    /// channel voice status or a system common status that waits for its
    /// data.
    status: Option<u8>,
    /// The data bytes read for `status`.
    data: Buf<2>,
    /// The SysEx in progress.
    sysex: Option<SysEx>,
}

/// A stream's state for a SysEx in progress.
#[derive(Debug, Clone, Copy)]
struct SysEx {
    /// The unsent bytes, at most one packet's worth.
    held: Buf<{ SysEx7::MAX_DATA }>,
    /// Whether the first packet of this SysEx was already sent.
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
    /// Returns a stream that sends its packets on `group`, 0 to 15.
    pub const fn new(group: u8) -> Self {
        ByteStream {
            group: group & 0xF,
            status: None,
            data: Buf::new(),
            sysex: None,
        }
    }

    /// Reads `bytes` and calls `send` with each packet once it is complete.
    ///
    /// Thread: any. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn read(&mut self, bytes: &[u8], mut send: impl FnMut([u32; 4])) {
        for &byte in bytes {
            self.byte(byte, &mut send);
        }
    }

    /// Forgets a message under way, a SysEx included, and any running
    /// status.
    fn reset(&mut self) {
        *self = ByteStream::new(self.group);
    }

    /// Ends a SysEx under way, then starts over, as after a device's error
    /// or its close: a SysEx that sent packets already ends with what the
    /// stream holds of it, so a reader holding it for its end is not left
    /// waiting, and one that sent none is dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_ump::ByteStream;
    ///
    /// let mut stream = ByteStream::new(0);
    /// let mut packets = Vec::new();
    /// // Seven bytes of a SysEx: six went out as its start.
    /// stream.read(&[0xF0, 1, 2, 3, 4, 5, 6, 7], |words| packets.push(words));
    /// assert_eq!(packets.len(), 1);
    /// stream.cut(|words| packets.push(words));
    /// // Its end, status 3, with the byte held.
    /// assert_eq!((packets[1][0] >> 20) & 0xF, 0x3);
    /// ```
    pub fn cut(&mut self, mut send: impl FnMut([u32; 4])) {
        if let Some(sysex) = self.sysex.take()
            && sysex.begun
        {
            self.finish(sysex, &mut send);
        }
        self.reset();
    }

    fn byte(&mut self, byte: u8, send: &mut impl FnMut([u32; 4])) {
        match byte {
            // A real-time byte is sent at once. 0xF9 and 0xFD are undefined
            // and dropped.
            0xF9 | 0xFD => {}
            0xF8..=0xFF => send(self.system(byte, 0, 0)),
            0x80..=0xF7 => self.status_byte(byte, send),
            _ => self.data_byte(byte, send),
        }
    }

    fn status_byte(&mut self, byte: u8, send: &mut impl FnMut([u32; 4])) {
        if let Some(sysex) = self.sysex.take() {
            self.finish(sysex, send);
            // An end of SysEx only finishes the SysEx. Any other status cuts
            // it short and is then read on its own.
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
            // A tune request has no data.
            Some(1) => send(self.system(byte, 0, 0)),
            // A channel voice status stays in force until another status. A
            // system common status waits for its data but doesn't become
            // running status.
            Some(_) => self.status = Some(byte),
            // A stray end of SysEx and the undefined 0xF4 and 0xF5 only end
            // the status in force.
            None => {}
        }
    }

    fn data_byte(&mut self, byte: u8, send: &mut impl FnMut([u32; 4])) {
        if let Some(sysex) = &mut self.sysex {
            if sysex.held.is_full() {
                // A seventh byte means that the six held bytes aren't the
                // last.
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
        // A kept status is 2 or 3 bytes long, including the status byte.
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
            // A system common message does not set running status.
            self.status = None;
        } else {
            send(word(MessageType::MIDI1, self.group, status, first, second));
        }
    }

    /// Sends the held bytes of `sysex` as its last packet, or as a complete
    /// packet if none was sent before.
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

/// Returns a one-word packet of type `kind` with a status byte and two data
/// bytes.
fn word(kind: MessageType, group: u8, status: u8, first: u8, second: u8) -> [u32; 4] {
    let head = Head {
        kind,
        group,
        status,
        byte3: first & 0x7F,
        byte4: second & 0x7F,
    };
    [head.word(), 0, 0, 0]
}

/// One packet as MIDI 1.0 bytes, for a device or port that takes the byte
/// stream.
///
/// It holds at most eight bytes, including a short SysEx's 0xF0 and 0xF7.
/// [`of`](Self::of) writes channel voice messages, system messages and
/// 7-bit SysEx, each in full without running status. A SysEx packet becomes
/// its part of the message, with 0xF0 if it is the first packet and 0xF7 if
/// it is the last.
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
/// // A MIDI 2.0 note-on must be downgraded first.
/// assert_eq!(Bytes::of([0x4090_3C00, 0xFFFF_0000, 0, 0]), None);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Bytes(Buf<8>);

impl Bytes {
    /// Returns `words` as MIDI 1.0 bytes.
    ///
    /// Returns `None` for utility and stream messages, 8-bit SysEx, undefined
    /// statuses and MIDI 2.0 messages. Convert a MIDI 2.0 message with
    /// [`Downgrade`](crate::Downgrade) first.
    ///
    /// Thread: any. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn of(words: [u32; 4]) -> Option<Bytes> {
        let [first, ..] = words;
        let head = Head::read(first);
        match head.kind {
            MessageType::SYSTEM if head.status >= 0xF0 => Bytes::short(head),
            MessageType::MIDI1 if head.status < 0xF0 => Bytes::short(head),
            MessageType::SYSEX7 => SysEx7::decode(words).map(Bytes::sysex),
            _ => None,
        }
    }

    /// Returns the length of a MIDI 1.0 message with `status`, including the
    /// status byte.
    ///
    /// A channel message is 3 bytes, or 2 for a program change or channel
    /// pressure. A song position pointer is 3 bytes, a time code quarter frame
    /// or a song select 2, and a tune request or a real-time message 1.
    /// Returns `None` for a data byte, an undefined status, and a SysEx start
    /// or end, because a SysEx's end byte sets its length.
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

    /// Returns a one-word packet's status and the data bytes that its length
    /// needs.
    fn short(head: Head) -> Option<Bytes> {
        let length = Bytes::length_of(head.status)?;
        let mut out = Bytes::default();
        let bytes = [head.status, head.byte3 & 0x7F, head.byte4 & 0x7F];
        for byte in bytes.into_iter().take(length) {
            out.0.push(byte);
        }
        Some(out)
    }

    /// Returns a SysEx packet's part of its message, with 0xF0 first if the
    /// packet begins the message and 0xF7 last if it ends it.
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
