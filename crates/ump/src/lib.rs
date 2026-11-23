//! Universal MIDI Packets, as every Hapto crate passes MIDI.
//!
//! An [`EventList`] is one [block]'s MIDI, in or out: a fixed-capacity list
//! of [`Event`]s. Each is one packet at a frame of the block.
//!
//! Everything above the exporters reads MIDI 2.0. An exporter upgrades MIDI
//! 1.0 on the way in with [`Upgrade`], and downgrades on the way out with
//! [`Downgrade`] only for a destination that takes MIDI 1.0 alone, both by
//! the specification's Default Translation. A packet keeps its
//! [`Origin`], so a value upgraded from MIDI 1.0 narrows back to what was
//! sent through [`scale`]. [`ChannelVoice`] reads and writes the MIDI 2.0
//! channel voice messages, [`Midi1Voice`] the MIDI 1.0 ones an exporter
//! translates, [`packet_words`] says how long a packet of any message type
//! is, and [`Packets`] splits a stream of words into events. Every packet
//! carries its own status, so MIDI 1.0 byte-stream running status never
//! applies: Hapto parses UMP only.
//!
//! Open: no exporter fills a block's list from the host's MIDI yet, so a
//! processor's list stays empty.
//!
//! The crate is in the core tier and depends on nothing. Every call but
//! those that make a list or an [`Upgrade`] is fit for the audio thread, and
//! none of them panics.
//!
//! [block]: https://github.com/hapto-dev/hapto/blob/main/docs/glossary.md#block
//!
//! # Examples
//!
//! A MIDI 1.0 control change upgraded, then read back as MIDI 2.0:
//!
//! ```
//! use hapto_ump::{ChannelMessage, ChannelVoice, EventList, Midi1Message, Midi1Voice, Upgrade, scale};
//!
//! let message = Midi1Message::ControlChange { controller: 7, value: 100 };
//! let sent = Midi1Voice { group: 0, channel: 2, message }.event(32);
//! let mut block = EventList::with_capacity(64);
//! Upgrade::new().block(&[sent], 63, &mut block);
//!
//! for event in block.as_slice() {
//!     // A packet Hapto does not read gives `None`: skip it.
//!     let Some(ChannelVoice { channel, message, .. }) = event.channel_voice() else {
//!         continue;
//!     };
//!     if let ChannelMessage::ControlChange { controller, value } = message {
//!         assert_eq!((event.offset, channel, controller), (32, 2, 7));
//!         // Upgraded from MIDI 1.0, it narrows back to what was sent.
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

mod downgrade;
mod midi1;
mod packets;
pub mod scale;
mod upgrade;
mod voice;

pub use downgrade::Downgrade;
pub use midi1::{Midi1Message, Midi1Voice};
pub use packets::{Packets, Truncated, packet_words};
pub use upgrade::Upgrade;
pub use voice::{Attribute, Bank, ChannelMessage, ChannelVoice};

/// One Universal MIDI Packet at a frame of the block.
///
/// # Examples
///
/// ```
/// use hapto_ump::{Attribute, ChannelMessage, ChannelVoice, Event};
///
/// // A MIDI 2.0 note-on at frame 16: note 60, full velocity.
/// let event = Event::new(16, [0x4090_3C00, 0xFFFF_0000, 0, 0]);
/// let message = ChannelMessage::NoteOn { note: 60, velocity: u16::MAX, attribute: Attribute::NONE };
/// assert_eq!(event.channel_voice(), Some(ChannelVoice { group: 0, channel: 0, message }));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
/// Everything above the exporters reads MIDI 2.0, so a packet's protocol is
/// no longer in its words. Its origin keeps what the words lost: a value
/// upgraded from MIDI 1.0 narrows back to its 7 or 14 bits exactly, as the
/// specification's round trip guarantees, for a reader that needs the
/// value as it was sent.
///
/// # Examples
///
/// ```
/// use hapto_ump::{Event, Origin};
///
/// // A MIDI 2.0 note-on, as a MIDI 2.0 source sent it.
/// let event = Event::new(0, [0x4090_3C00, 0xFFFF_0000, 0, 0]);
/// assert_eq!(event.origin, Origin::Midi2);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Origin {
    /// Sent as it is: a MIDI 2.0 packet, or one both protocols share, such
    /// as SysEx7 or a system message.
    #[default]
    Midi2,
    /// Sent as MIDI 1.0 and upgraded on the way in.
    Midi1,
}

impl Event {
    /// The packet `words` at frame `offset`, sent as it is.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub const fn new(offset: u32, words: [u32; 4]) -> Self {
        Event {
            offset,
            words,
            origin: Origin::Midi2,
        }
    }

    /// The packet's message type, 0 to 15: 2 for a MIDI 1.0 channel voice
    /// message, 3 for SysEx7, 4 for a MIDI 2.0 channel voice message.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_ump::Event;
    ///
    /// let note_on = Event::new(0, [0x2090_3C7F, 0, 0, 0]);
    /// let sysex = Event::new(0, [0x3516_7E7F, 0x0D70_0201, 0, 0]);
    /// assert_eq!(note_on.message_type(), 2);
    /// assert_eq!(sysex.message_type(), 3);
    /// ```
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn message_type(&self) -> u8 {
        (self.words[0] >> 28) as u8
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
        ((self.words[0] >> 24) & 0xF) as u8
    }

    /// The MIDI 2.0 channel voice message this packet carries, as
    /// [`ChannelVoice::decode`] reads it.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn channel_voice(&self) -> Option<ChannelVoice> {
        ChannelVoice::decode(self.words)
    }

    /// The MIDI 1.0 channel voice message this packet carries, as
    /// [`Midi1Voice::decode`] reads it: for an exporter's translation, since
    /// nothing above it reads MIDI 1.0.
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
/// use hapto_ump::{Event, EventList};
///
/// let mut list = EventList::with_capacity(8);
/// // A MIDI 1.0 note-on at frame 16.
/// list.push(Event::new(16, [0x2090_3C7F, 0, 0, 0]));
///
/// for event in list.as_slice() {
///     assert_eq!((event.offset, event.message_type()), (16, 2));
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
        assert_eq!(sysex.message_type(), 3);
        assert_eq!(sysex.group(), 5);
        assert_eq!(sysex.channel_voice(), None);
    }
}
