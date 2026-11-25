//! Defines the UMP message type and splits a stream of words into packets.

use std::fmt;
use std::iter::FusedIterator;

use crate::Event;

/// A packet's message type, which says what the packet carries and how many
/// words it takes.
///
/// It is the top four bits of the first word. The named types are the ones
/// in the UMP 1.1 table. The reserved types have sizes too, so code can skip
/// a packet of an unknown type.
///
/// # Examples
///
/// ```
/// use hapto_ump::{Event, MessageType};
///
/// let note_on = Event::new(0, [0x2090_3C7F, 0, 0, 0]);
/// match note_on.message_type() {
///     MessageType::MIDI1 | MessageType::MIDI2 => {} // a channel voice message
///     other => panic!("{other:?} is no note"),
/// }
/// assert_eq!(MessageType::MIDI2.words(), 2);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MessageType(u8);

impl MessageType {
    /// Utility messages, such as jitter-reduction timestamps, in one word
    /// with no group.
    pub const UTILITY: Self = MessageType(0x0);
    /// System real-time and system common messages, in one word.
    pub const SYSTEM: Self = MessageType(0x1);
    /// MIDI 1.0 channel voice messages, [`Midi1Voice`](crate::Midi1Voice),
    /// in one word.
    pub const MIDI1: Self = MessageType(0x2);
    /// 7-bit System Exclusive, [`SysEx7`](crate::SysEx7), in two words.
    pub const SYSEX7: Self = MessageType(0x3);
    /// MIDI 2.0 channel voice messages,
    /// [`ChannelVoice`](crate::ChannelVoice), in two words.
    pub const MIDI2: Self = MessageType(0x4);
    /// 8-bit System Exclusive and mixed data sets, in four words.
    pub const SYSEX8: Self = MessageType(0x5);
    /// Flex data, such as tempo and lyrics, in four words.
    pub const FLEX_DATA: Self = MessageType(0xD);
    /// UMP stream messages, which describe the endpoint, in four words with
    /// no group.
    pub const STREAM: Self = MessageType(0xF);

    /// Returns the type of a packet whose first word is `first`.
    pub const fn of(first: u32) -> Self {
        MessageType((first >> 28) as u8)
    }

    /// Returns the type's number, 0 to 15.
    pub const fn get(self) -> u8 {
        self.0
    }

    /// Returns the packet length in 32-bit words, 1 to 4, from the UMP 1.1
    /// table.
    pub const fn words(self) -> usize {
        match self.0 {
            0x0..=0x2 | 0x6 | 0x7 => 1,
            0x3 | 0x4 | 0x8..=0xA => 2,
            0xB | 0xC => 3,
            _ => 4,
        }
    }
}

/// A stream ended inside a packet: its first word promised more words than
/// the stream has left.
///
/// # Examples
///
/// ```
/// use hapto_ump::Packets;
///
/// // A MIDI 2.0 note-on is two words; the stream has only its first.
/// let mut packets = Packets::new(&[0x4090_3C00], 0);
/// let cut = packets.next().expect("one result").expect_err("the packet is cut");
/// assert_eq!((cut.message_type.get(), cut.needed, cut.available), (4, 2, 1));
/// assert_eq!(cut.to_string(), "a type 0x4 packet takes 2 words and 1 were left");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Truncated {
    /// The packet's message type.
    pub message_type: MessageType,
    /// The words that type takes.
    pub needed: usize,
    /// The words that were left.
    pub available: usize,
}

impl fmt::Display for Truncated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a type {:#X} packet takes {} words and {} were left",
            self.message_type.get(),
            self.needed,
            self.available
        )
    }
}

impl std::error::Error for Truncated {}

/// A stream of 32-bit words split into packets, as [`Event`]s.
///
/// Every packet becomes an event at the offset given to
/// [`new`](Self::new), with the words it has and zero for the rest. A
/// packet the stream's end cuts short gives a [`Truncated`], the last item.
///
/// # Examples
///
/// ```
/// use hapto_ump::Packets;
///
/// // A MIDI 1.0 note-on, then a MIDI 2.0 note-on cut after its first word.
/// let stream = [0x2090_3C7F, 0x4090_3C00];
/// let mut packets = Packets::new(&stream, 32);
/// assert_eq!(packets.next().expect("a packet").expect("whole").words[0], 0x2090_3C7F);
/// let cut = packets.next().expect("a result").expect_err("cut");
/// assert_eq!((cut.needed, cut.available), (2, 1));
/// assert_eq!(packets.next(), None);
/// ```
#[derive(Debug, Clone)]
pub struct Packets<'a> {
    words: &'a [u32],
    offset: u32,
}

impl<'a> Packets<'a> {
    /// Splits `words`, giving every event `offset`.
    pub fn new(words: &'a [u32], offset: u32) -> Self {
        Packets { words, offset }
    }
}

impl Iterator for Packets<'_> {
    type Item = Result<Event, Truncated>;

    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    fn next(&mut self) -> Option<Self::Item> {
        let first = *self.words.first()?;
        let message_type = MessageType::of(first);
        let needed = message_type.words();
        let Some((packet, rest)) = self.words.split_at_checked(needed) else {
            let available = self.words.len();
            self.words = &[];
            return Some(Err(Truncated {
                message_type,
                needed,
                available,
            }));
        };
        let mut words = [0; 4];
        for (slot, word) in words.iter_mut().zip(packet) {
            *slot = *word;
        }
        self.words = rest;
        Some(Ok(Event::new(self.offset, words)))
    }
}

// An empty stream stays empty, so `None` repeats.
impl FusedIterator for Packets<'_> {}
