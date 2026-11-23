//! Defines 7-bit System Exclusive packets, which carry a SysEx message six
//! bytes at a time.

use crate::buf::Buf;
use crate::head::Head;
use crate::{Event, MessageType};

/// The bytes that frame a SysEx in MIDI 1.0's byte stream.
pub(crate) const SYSEX_BEGIN: u8 = 0xF0;
pub(crate) const SYSEX_FINISH: u8 = 0xF7;

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
    /// The whole message.
    Complete,
    /// The first packet, with more to follow.
    Start,
    /// A middle packet.
    Continue,
    /// The last packet.
    End,
}

impl SysExForm {
    /// Returns whether this form begins a message, as `Complete` and `Start`
    /// do.
    pub const fn begins(self) -> bool {
        matches!(self, SysExForm::Complete | SysExForm::Start)
    }

    /// Returns whether this form ends a message, as `Complete` and `End` do.
    pub const fn ends(self) -> bool {
        matches!(self, SysExForm::Complete | SysExForm::End)
    }

    /// Returns the form of packet `index`, from 0, in a message of `count`
    /// packets.
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

    const fn status(self) -> u8 {
        match self {
            SysExForm::Complete => 0x0,
            SysExForm::Start => 0x1,
            SysExForm::Continue => 0x2,
            SysExForm::End => 0x3,
        }
    }

    /// Returns the form that a status names, or `None` for the reserved 4 to
    /// 15.
    const fn from_status(status: u8) -> Option<SysExForm> {
        match status {
            0x0 => Some(SysExForm::Complete),
            0x1 => Some(SysExForm::Start),
            0x2 => Some(SysExForm::Continue),
            0x3 => Some(SysExForm::End),
            _ => None,
        }
    }
}

/// One 7-bit System Exclusive packet, message type 3.
///
/// A packet carries up to six bytes of a SysEx message, its group and its
/// [`SysExForm`]. The bytes are 7-bit data only, without the 0xF0 and 0xF7
/// that frame a SysEx in the MIDI 1.0 byte stream. Both protocols carry
/// SysEx this way.
///
/// # Examples
///
/// ```
/// use hapto_ump::{SysEx7, SysExForm};
///
/// // An identity request fits in one packet on group 0.
/// let packet = SysEx7::new(0, SysExForm::Complete, &[0x7E, 0x7F, 0x06, 0x01]);
/// assert_eq!(packet.words(), [0x3004_7E7F, 0x0601_0000, 0, 0]);
/// assert_eq!(SysEx7::decode(packet.words()), Some(packet));
/// assert_eq!(packet.data(), [0x7E, 0x7F, 0x06, 0x01]);
///
/// // Twenty bytes take a start, two continues and an end.
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
    /// The group, 0 to 15.
    pub group: u8,
    /// Where the bytes fall in their message.
    pub form: SysExForm,
    data: Buf<6>,
}

impl SysEx7 {
    /// The most bytes that one packet carries.
    pub const MAX_DATA: usize = 6;

    /// Returns a packet on `group`, 0 to 15, with the first six bytes of
    /// `data`, each masked to 7 bits.
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

    /// Returns the packet that `words` carries, or `None` for another
    /// message type or a reserved status.
    ///
    /// A byte count above six is read as six.
    ///
    /// Thread: any. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn decode(words: [u32; 4]) -> Option<Self> {
        let [first, second, ..] = words;
        let head = Head::read(first);
        if head.kind != MessageType::SYSEX7 {
            return None;
        }
        // The status byte holds the form and then the byte count.
        let form = SysExForm::from_status(head.status >> 4)?;
        let count = usize::from(head.status & 0xF).min(Self::MAX_DATA);
        let [b3, b4, b5, b6] = second.to_be_bytes();
        let bytes = [head.byte3, head.byte4, b3, b4, b5, b6];
        let data = bytes.get(..count).unwrap_or(&[]);
        Some(SysEx7::new(head.group, form, data))
    }

    /// Returns the bytes, at most six.
    pub fn data(&self) -> &[u8] {
        self.data.as_slice()
    }

    /// Returns the packet's words.
    pub fn words(&self) -> [u32; 4] {
        let mut bytes = [0_u8; 6];
        for (slot, &byte) in bytes.iter_mut().zip(self.data()) {
            *slot = byte;
        }
        let [b1, b2, b3, b4, b5, b6] = bytes;
        let count = self.data().len() as u8;
        let head = Head {
            kind: MessageType::SYSEX7,
            group: self.group,
            status: self.form.status() << 4 | count,
            byte3: b1,
            byte4: b2,
        };
        [head.word(), u32::from_be_bytes([b3, b4, b5, b6]), 0, 0]
    }

    /// Returns the packet as an event at frame `offset`.
    pub fn event(&self, offset: u32) -> Event {
        Event::new(offset, self.words())
    }

    /// Splits a whole message's `data` into packets on `group` and calls
    /// `send` with each one's words in order.
    ///
    /// Up to six bytes, including none, make one complete packet, and more
    /// make a start, continues and an end. Each byte is masked to 7 bits.
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

    /// Returns the number of packets that `len` bytes take, which is one for
    /// up to six bytes, including none.
    pub const fn count(len: usize) -> usize {
        let count = len.div_ceil(Self::MAX_DATA);
        if count == 0 { 1 } else { count }
    }
}

impl Event {
    /// Returns the event's SysEx7 packet, as [`SysEx7::decode`] reads it.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn sysex7(&self) -> Option<SysEx7> {
        SysEx7::decode(self.words)
    }
}

/// Joins 7-bit SysEx packets back into whole messages, by the rules of
/// section 7.7.1 of the UMP and MIDI 2.0 Protocol specification, in room
/// fixed when it is made.
///
/// It reads every packet of a stream, so it can apply each rule:
///
/// - A SysEx goes out once its last packet has come, framed by 0xF0 and
///   0xF7, and lasts until the next packet is pushed.
/// - One SysEx is under way at a time. Another group's, begun while one is
///   under way, is dropped, counted at its start, and the rest of it passes
///   unread.
/// - Any other message on the SysEx's group, a new SysEx's start or a
///   packet with a reserved status included, ends the one under way: it is
///   dropped, and the rest of it passes unread. Real-time messages and the
///   groupless utility and stream messages may fall inside it.
/// - A SysEx whose start never came, or one longer than the room, is
///   dropped, counted at its end.
///
/// # Examples
///
/// ```
/// use hapto_ump::{Event, SysEx7, SysExJoin};
///
/// let mut join = SysExJoin::with_room(64);
/// let mut packets = Vec::new();
/// SysEx7::packets(0, &[1, 2, 3, 4, 5, 6, 7, 8], |words| packets.push(Event::new(0, words)));
///
/// // The first packet holds six bytes, and is no message yet.
/// assert!(join.push(&packets[0]).whole.is_none());
/// let joined = join.push(&packets[1]);
/// let whole = joined.whole.expect("the last packet");
/// assert_eq!(whole.data(), [1, 2, 3, 4, 5, 6, 7, 8]);
/// assert_eq!(whole.framed().first(), Some(&0xF0));
/// assert_eq!(joined.dropped, 0);
/// ```
#[derive(Debug)]
pub struct SysExJoin {
    /// 0xF0, the bytes so far, and 0xF7 once whole.
    bytes: Vec<u8>,
    /// The most data bytes a message may have.
    room: usize,
    state: State,
    /// The groups whose SysEx was dropped part way, a bit each: the rest
    /// of it passes unread, to its end.
    skipping: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    /// A SysEx under way on `group`; `over` once it outgrew the room.
    Joining {
        group: u8,
        over: bool,
    },
    /// A SysEx whole on `group`, until the next packet.
    Whole {
        group: u8,
    },
}

/// What one packet came to: a SysEx it made whole, and how many it dropped.
///
/// # Examples
///
/// ```
/// use hapto_ump::{Event, SysEx7, SysExForm, SysExJoin};
///
/// let mut join = SysExJoin::with_room(64);
/// join.push(&SysEx7::new(0, SysExForm::Start, &[1; 6]).event(0));
/// // A note-on on the SysEx's group ends it, cut short.
/// let joined = join.push(&Event::new(0, [0x2090_3C7F, 0, 0, 0]));
/// assert_eq!((joined.whole, joined.dropped), (None, 1));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Joined<'a> {
    /// The message the packet made whole.
    pub whole: Option<SysExMessage<'a>>,
    /// How many messages the packet dropped: one at most, or two when a
    /// new SysEx on the group of one under way is dropped too.
    pub dropped: u32,
}

/// A whole SysEx message, as [`SysExJoin`] joined it.
///
/// # Examples
///
/// ```
/// use hapto_ump::{SysEx7, SysExForm, SysExJoin};
///
/// let mut join = SysExJoin::with_room(64);
/// let packet = SysEx7::new(0, SysExForm::Complete, &[0x7E, 0x7F]);
/// let message = join.push(&packet.event(0)).whole.expect("whole in one packet");
/// assert_eq!(message.data(), [0x7E, 0x7F]);
/// assert_eq!(message.framed(), [0xF0, 0x7E, 0x7F, 0xF7]);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SysExMessage<'a> {
    /// The group it came on, 0 to 15.
    pub group: u8,
    framed: &'a [u8],
}

impl<'a> SysExMessage<'a> {
    /// Its bytes, without the 0xF0 and 0xF7 that frame it.
    pub fn data(&self) -> &'a [u8] {
        let inner = self
            .framed
            .strip_prefix(&[SYSEX_BEGIN])
            .unwrap_or(self.framed);
        inner.strip_suffix(&[SYSEX_FINISH]).unwrap_or(inner)
    }

    /// Its bytes framed by 0xF0 and 0xF7, as MIDI 1.0's byte stream and
    /// most formats carry SysEx.
    pub fn framed(&self) -> &'a [u8] {
        self.framed
    }
}

impl SysExJoin {
    /// A join for SysEx of up to `room` bytes, not counting the 0xF0 and
    /// 0xF7 that frame them. Allocates, so it is made before the audio
    /// thread uses it.
    ///
    /// Thread: any but audio.
    pub fn with_room(room: usize) -> Self {
        SysExJoin {
            bytes: Vec::with_capacity(room.saturating_add(2)),
            room,
            state: State::Idle,
            skipping: 0,
        }
    }

    /// Reads the next packet of the stream.
    ///
    /// Thread: any. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn push(&mut self, event: &Event) -> Joined<'_> {
        if let State::Whole { .. } = self.state {
            self.forget();
        }
        let dropped = match event.sysex7() {
            Some(packet) => self.part(packet),
            None => self.other(event),
        };
        Joined {
            whole: self.message(),
            dropped,
        }
    }

    /// The message the last packet made whole, until the next is pushed.
    pub fn message(&self) -> Option<SysExMessage<'_>> {
        match self.state {
            State::Whole { group } => Some(SysExMessage {
                group,
                framed: &self.bytes,
            }),
            _ => None,
        }
    }

    /// The group of the SysEx under way, if one is.
    pub fn under_way(&self) -> Option<u8> {
        match self.state {
            State::Joining { group, .. } => Some(group),
            _ => None,
        }
    }

    /// Forgets a SysEx under way or whole, as when its source starts over.
    pub fn reset(&mut self) {
        self.forget();
        self.skipping = 0;
    }

    /// A packet that is no SysEx7 packet: it ends a SysEx on its group,
    /// unless it may fall inside one.
    fn other(&mut self, event: &Event) -> u32 {
        let groupless = matches!(
            event.message_type(),
            MessageType::UTILITY | MessageType::STREAM
        );
        match self.state {
            State::Joining { group, .. }
                if !groupless && !event.is_real_time() && group == event.group() =>
            {
                self.forget();
                self.skipping |= bit(group);
                1
            }
            _ => 0,
        }
    }

    /// A SysEx7 packet, returning how many messages it dropped.
    fn part(&mut self, packet: SysEx7) -> u32 {
        let own = bit(packet.group);
        if packet.form.begins() {
            self.skipping &= !own;
        } else if self.skipping & own != 0 {
            // The rest of a SysEx dropped part way, counted already.
            if packet.form.ends() {
                self.skipping &= !own;
            }
            return 0;
        }
        let mut dropped: u32 = 0;
        if let State::Joining { group, .. } = self.state {
            if group != packet.group {
                if !packet.form.begins() {
                    // A part of a SysEx whose start never came.
                    return u32::from(packet.form.ends());
                }
                // One at a time: another group's is dropped at its start,
                // and the rest of it passes unread.
                if !packet.form.ends() {
                    self.skipping |= own;
                }
                return 1;
            }
            if packet.form.begins() {
                // A new one on its group ends the one under way.
                self.forget();
                dropped = 1;
            }
        }
        if packet.form.begins() {
            self.bytes.push(SYSEX_BEGIN);
            self.state = State::Joining {
                group: packet.group,
                over: false,
            };
        }
        let State::Joining { group, over } = self.state else {
            // A part of a SysEx whose start never came.
            return dropped.saturating_add(u32::from(packet.form.ends()));
        };
        let held = self.bytes.len().saturating_sub(1);
        let over = over || held.saturating_add(packet.data().len()) > self.room;
        if !over {
            self.bytes.extend_from_slice(packet.data());
        }
        if !packet.form.ends() {
            self.state = State::Joining { group, over };
            return dropped;
        }
        if over {
            self.forget();
            return dropped.saturating_add(1);
        }
        self.bytes.push(SYSEX_FINISH);
        self.state = State::Whole { group };
        dropped
    }

    fn forget(&mut self) {
        self.bytes.clear();
        self.state = State::Idle;
    }
}

/// `group`'s bit in a mask of the 16 groups.
const fn bit(group: u8) -> u16 {
    match 1_u16.checked_shl((group & 0xF) as u32) {
        Some(bit) => bit,
        None => 0,
    }
}
