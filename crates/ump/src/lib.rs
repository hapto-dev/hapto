//! Universal MIDI Packets, as every Hapto crate passes MIDI.
//!
//! An [`EventList`] is a fixed-capacity list of [`Event`]s that holds one
//! [block]'s MIDI, in or out. Each event is one packet at a frame of the
//! block.
//!
//! Everything above the exporters reads MIDI 2.0. An exporter upgrades
//! incoming MIDI 1.0 with [`Upgrade`], and uses [`Downgrade`] only for a
//! destination that accepts only MIDI 1.0. Both follow the specification's
//! Default Translation. Each packet keeps its [`Origin`], so [`scale`] can
//! narrow an upgraded value back to what was sent.
//!
//! Every packet carries its own status, so Hapto parses only UMP. These types
//! read and write it:
//!
//! - [`ChannelVoice`] for MIDI 2.0 channel voice messages.
//! - [`Midi1Voice`] for MIDI 1.0 ones, which an exporter translates.
//! - [`SysEx7`] for System Exclusive, which both protocols share.
//! - [`MessageType`] for a packet's kind and length, and [`Packets`] to split
//!   a stream of words into events.
//! - [`ByteStream`] and [`Bytes`] for a MIDI 1.0 device's bytes, at the edge.
//!
//! The crate is in the core tier and depends on nothing. Every call except
//! those that make a list or an [`Upgrade`] can run on the audio thread, and
//! none panics.
//!
//! [block]: https://github.com/hapto-dev/hapto/blob/main/docs/glossary.md#block
//!
//! # Examples
//!
//! Upgrade a MIDI 1.0 control change, then read it back as MIDI 2.0:
//!
//! ```
//! use hapto_ump::{ChannelMessage, ChannelVoice, EventList, Midi1Message, Midi1Voice, Upgrade, scale};
//!
//! let message = Midi1Message::ControlChange { controller: 7, value: 100 };
//! let sent = Midi1Voice { group: 0, channel: 2, message }.event(32);
//! let mut block = EventList::with_capacity(64);
//! Upgrade::new().block(&[sent], &mut block);
//!
//! for event in block.as_slice() {
//!     // A packet Hapto does not read gives `None`: skip it.
//!     let Some(ChannelVoice { channel, message, .. }) = event.channel_voice() else {
//!         continue;
//!     };
//!     if let ChannelMessage::ControlChange { controller, value } = message {
//!         assert_eq!((event.offset, channel, controller), (32, 2, 7));
//!         // `scale` narrows the upgraded value back to what was sent.
//!         assert_eq!(scale::down_32_to_7(value), 100);
//!     }
//! }
//! ```

// The nightly real-time sanitizer lane marks the calls the audio thread makes.
#![cfg_attr(hapto_rtsan, feature(sanitize))]
// A panic on the audio path ends a host, so the crate refuses indexing,
// unwrap, expect, panic and unchecked arithmetic outside its unit tests.
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

mod buf;
mod bytes;
pub mod controller;
mod downgrade;
mod head;
mod midi1;
mod packets;
pub mod scale;
mod sysex;
mod upgrade;
mod voice;

pub use bytes::{ByteStream, Bytes};
pub use downgrade::Downgrade;
pub use midi1::{Midi1Message, Midi1Voice};
pub use packets::{MessageType, Packets, Truncated};
pub use sysex::{Joined, SysEx7, SysExForm, SysExJoin, SysExMessage};
pub use upgrade::Upgrade;
pub use voice::{Attribute, Bank, ChannelMessage, ChannelVoice};

/// One Universal MIDI Packet at a frame of the block.
///
/// # Examples
///
/// ```
/// use hapto_ump::{Attribute, ChannelMessage, ChannelVoice, Event};
///
/// // A MIDI 2.0 note-on for note 60 at full velocity, at frame 16.
/// let event = Event::new(16, [0x4090_3C00, 0xFFFF_0000, 0, 0]);
/// let message = ChannelMessage::NoteOn { note: 60, velocity: u16::MAX, attribute: Attribute::NONE };
/// assert_eq!(event.channel_voice(), Some(ChannelVoice { group: 0, channel: 0, message }));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Event {
    /// The frame it happens at, from the start of the block.
    pub offset: u32,
    /// The packet: one to four words, unused words zero.
    pub words: [u32; 4],
    /// Where the packet came from before it reached the block.
    pub origin: Origin,
}

/// Where a packet came from before it reached a block.
///
/// An upgraded packet's words don't show which protocol sent it, so the origin
/// records it for a reader that needs a value as it was sent. See [`scale`].
///
/// # Examples
///
/// ```
/// use hapto_ump::{Event, Origin};
///
/// // A note-on that a MIDI 2.0 source sent.
/// let event = Event::new(0, [0x4090_3C00, 0xFFFF_0000, 0, 0]);
/// assert_eq!(event.origin, Origin::Midi2);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Origin {
    /// Sent unchanged, as a MIDI 2.0 packet or one that both protocols
    /// share, such as SysEx7 or a system message.
    #[default]
    Midi2,
    /// Sent as MIDI 1.0 and upgraded on the way in.
    Midi1,
    /// Sent as an RPN or NRPN whose data entry high byte came without a low
    /// byte at its frame, and upgraded as if the low byte were 0. A low byte
    /// at a later frame arrives as a separate controller marked
    /// [`Midi1`](Self::Midi1), so a reader can tell a sent low byte of 0 from
    /// a missing one.
    Midi1HighByte,
    /// Sent as a format's own note event, which named the note: VST3's
    /// `noteId`. The packet is the note's message on its key, and `id` is
    /// the host's name for the note, which the words have no room for.
    ///
    /// ```
    /// use hapto_ump::{Event, MessageType, Origin};
    ///
    /// // A note-on whose host named its note 7.
    /// let event = Event {
    ///     origin: Origin::Note { id: 7 },
    ///     ..Event::new(0, [0x4090_3C00, 0xFFFF_0000, 0, 0])
    /// };
    /// assert_eq!(event.message_type(), MessageType::MIDI2);
    /// ```
    Note {
        /// The host's id for the note, below `0x8000_0000`.
        id: u32,
    },
}

impl Event {
    /// Returns an event for the packet `words` at frame `offset`, with origin
    /// [`Origin::Midi2`].
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub const fn new(offset: u32, words: [u32; 4]) -> Self {
        Event {
            offset,
            words,
            origin: Origin::Midi2,
        }
    }

    /// The offset of the last frame of a block of `frames` frames, where a
    /// packet at or past the block's end goes: 0 for a block of none, and
    /// the last offset there is for a block longer than offsets reach.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_ump::Event;
    ///
    /// assert_eq!(Event::last_offset(512), 511);
    /// assert_eq!(Event::last_offset(0), 0);
    /// ```
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn last_offset(frames: usize) -> u32 {
        u32::try_from(frames.saturating_sub(1)).unwrap_or(u32::MAX)
    }

    /// The packet's message type.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_ump::{Event, MessageType};
    ///
    /// let note_on = Event::new(0, [0x2090_3C7F, 0, 0, 0]);
    /// let sysex = Event::new(0, [0x3516_7E7F, 0x0D70_0201, 0, 0]);
    /// assert_eq!(note_on.message_type(), MessageType::MIDI1);
    /// assert_eq!(sysex.message_type(), MessageType::SYSEX7);
    /// ```
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn message_type(&self) -> MessageType {
        MessageType::of(self.words[0])
    }

    /// Returns whether the packet is a System Real Time message (status 0xF8
    /// to 0xFF).
    ///
    /// Such a message can arrive anywhere, even inside a SysEx.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_ump::Event;
    ///
    /// assert!(Event::new(0, [0x10F8_0000, 0, 0, 0]).is_real_time()); // the clock
    /// assert!(!Event::new(0, [0x10F2_0000, 0, 0, 0]).is_real_time()); // song position
    /// ```
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn is_real_time(&self) -> bool {
        let head = head::Head::read(self.words[0]);
        head.kind == MessageType::SYSTEM && head.status >= 0xF8
    }

    /// The packet's group, 0 to 15. Meaningless for the message types that
    /// have none (utility and stream messages).
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_ump::Event;
    ///
    /// // A MIDI 1.0 note-on on group 5.
    /// let event = Event::new(0, [0x2590_3C7F, 0, 0, 0]);
    /// assert_eq!(event.group(), 5);
    /// ```
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn group(&self) -> u8 {
        head::Head::read(self.words[0]).group
    }

    /// Returns the packet's MIDI 2.0 channel voice message, decoded with
    /// [`ChannelVoice::decode`].
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn channel_voice(&self) -> Option<ChannelVoice> {
        ChannelVoice::decode(self.words)
    }

    /// Returns the packet's MIDI 1.0 channel voice message, decoded with
    /// [`Midi1Voice::decode`], for an exporter to translate.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn midi1_voice(&self) -> Option<Midi1Voice> {
        Midi1Voice::decode(self.words)
    }
}

/// One block's MIDI events, in or out. Events in come in the order the host
/// delivered them (formats deliver them sorted by offset); events out go in
/// the order the processor pushed them.
///
/// Its capacity is fixed when it is made, and it never grows: an event
/// pushed past the capacity is dropped and counted.
///
/// # Examples
///
/// ```
/// use hapto_ump::{Event, EventList, MessageType};
///
/// let mut list = EventList::with_capacity(8);
/// // A MIDI 1.0 note-on at frame 16.
/// list.push(Event::new(16, [0x2090_3C7F, 0, 0, 0]));
///
/// for event in list.as_slice() {
///     assert_eq!((event.offset, event.message_type()), (16, MessageType::MIDI1));
/// }
/// assert_eq!(list.dropped(), 0);
/// ```
#[derive(Debug)]
pub struct EventList {
    events: Box<[Event]>,
    len: usize,
    dropped: u32,
}

impl EventList {
    /// An empty list with room for `capacity` events. Allocates, so a plugin
    /// makes its lists when it activates.
    ///
    /// Thread: any but audio.
    pub fn with_capacity(capacity: usize) -> Self {
        EventList {
            events: vec![Event::default(); capacity].into_boxed_slice(),
            len: 0,
            dropped: 0,
        }
    }

    /// Empties the list and its dropped count for the next block, keeping its
    /// storage.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_ump::{Event, EventList};
    ///
    /// let mut list = EventList::with_capacity(1);
    /// let event = Event::new(0, [0x2090_3C7F, 0, 0, 0]);
    /// list.push(event);
    /// list.push(event); // full: dropped and counted
    /// assert_eq!(list.dropped(), 1);
    ///
    /// list.clear();
    /// assert!(list.as_slice().is_empty());
    /// assert_eq!(list.dropped(), 0);
    /// assert_eq!(list.capacity(), 1); // the storage stays
    /// ```
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn clear(&mut self) {
        self.len = 0;
        self.dropped = 0;
    }

    /// Adds an event, or drops and counts it when the list is full.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_ump::{Event, EventList};
    ///
    /// let mut list = EventList::with_capacity(1);
    /// list.push(Event::new(0, [0x2090_3C7F, 0, 0, 0]));
    /// list.push(Event::new(8, [0x2080_3C00, 0, 0, 0]));
    /// assert_eq!(list.as_slice().len(), 1);
    /// assert_eq!(list.dropped(), 1);
    /// ```
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn push(&mut self, event: Event) {
        match self.events.get_mut(self.len) {
            Some(slot) => {
                *slot = event;
                // Below the events' length, since a slot was found there.
                self.len = self.len.saturating_add(1);
            }
            None => self.dropped = self.dropped.saturating_add(1),
        }
    }

    /// The events, in delivery order.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn as_slice(&self) -> &[Event] {
        // `len` never passes the events' length, so this is never empty
        // when events were pushed.
        self.events.get(..self.len).unwrap_or_default()
    }

    /// The events, in delivery order, to change in place.
    ///
    /// # Examples
    ///
    /// An exporter that plays a long block in two pieces moves the events
    /// the second piece sent to the block's own frames:
    ///
    /// ```
    /// use hapto_ump::{Event, EventList};
    ///
    /// let mut out = EventList::with_capacity(4);
    /// // The second piece starts at frame 64 and sends a note-on at its 3rd.
    /// out.push(Event::new(3, [0x2090_3C7F, 0, 0, 0]));
    /// for event in out.as_mut_slice() {
    ///     event.offset += 64;
    /// }
    /// assert_eq!(out.as_slice()[0].offset, 67);
    /// ```
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn as_mut_slice(&mut self) -> &mut [Event] {
        // As `as_slice`.
        self.events.get_mut(..self.len).unwrap_or_default()
    }

    /// How many events this block dropped for want of room.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn dropped(&self) -> u32 {
        self.dropped
    }

    /// Adds `count` to the events this block dropped, saturating at
    /// `u32::MAX`. A list that holds a copy of another's events takes the
    /// other's drops this way, so a reader of the copy still learns of them.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_ump::{Event, EventList};
    ///
    /// // The host's block had room for one event and was sent two.
    /// let mut block = EventList::with_capacity(1);
    /// block.push(Event::new(0, [0x2090_3C7F, 0, 0, 0]));
    /// block.push(Event::new(8, [0x2080_3C00, 0, 0, 0]));
    ///
    /// // A copy of it reports the note-off the block lost.
    /// let mut copy = EventList::with_capacity(1);
    /// for &event in block.as_slice() {
    ///     copy.push(event);
    /// }
    /// copy.add_dropped(block.dropped());
    /// assert_eq!(copy.dropped(), 1);
    /// ```
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn add_dropped(&mut self, count: u32) {
        self.dropped = self.dropped.saturating_add(count);
    }

    /// The most events the list holds.
    pub fn capacity(&self) -> usize {
        self.events.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_list_drops_and_counts_instead_of_growing() {
        let mut list = EventList::with_capacity(2);
        for offset in 0..3 {
            list.push(Event::new(offset, [0x2090_3C7F, 0, 0, 0]));
        }
        assert_eq!(list.as_slice().len(), 2);
        assert_eq!(list.dropped(), 1);
        assert_eq!(list.capacity(), 2);
        list.clear();
        assert!(list.as_slice().is_empty());
        assert_eq!(list.dropped(), 0);
    }

    #[test]
    fn events_changed_in_place_keep_the_list_s_length_and_drops() {
        let mut list = EventList::with_capacity(2);
        for offset in [1, 2, 3] {
            list.push(Event::new(offset, [0x2090_3C7F, 0, 0, 0]));
        }
        for event in list.as_mut_slice() {
            event.offset += 10;
        }
        let offsets: Vec<u32> = list.as_slice().iter().map(|event| event.offset).collect();
        assert_eq!(offsets, [11, 12]);
        assert_eq!(list.dropped(), 1, "the third, which had no room");
        assert!(EventList::with_capacity(4).as_mut_slice().is_empty());
    }

    #[test]
    fn added_drops_count_with_the_list_s_own_and_saturate() {
        let mut list = EventList::with_capacity(0);
        list.push(Event::default());
        list.add_dropped(2);
        assert_eq!(list.dropped(), 3);
        list.add_dropped(u32::MAX);
        assert_eq!(list.dropped(), u32::MAX);
        list.clear();
        assert_eq!(list.dropped(), 0, "a new block starts at none");
    }

    #[test]
    fn the_header_names_the_type_and_group() {
        let sysex = Event::new(0, [0x3516_7E7F, 0x0D70_0201, 0, 0]);
        assert_eq!(sysex.message_type(), MessageType::SYSEX7);
        assert_eq!(sysex.group(), 5);
        assert_eq!(sysex.channel_voice(), None);
    }
}
