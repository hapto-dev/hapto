//! The MIDI that a block receives, decoded once into messages at their
//! frames, and the MIDI that it sends.

use std::fmt;

use hapto_ump::{Event, EventList, Origin, SysEx7};

use crate::decoder::{Decoded, Decoder};
use crate::encode::encode;
use crate::message::{Message, NoteOff};
use crate::values::Group;

/// How many messages a list made with [`MidiList::new`] holds in a block.
pub const MESSAGES: usize = 2048;
/// How many SysEx bytes, in total, that list holds in a block.
pub const BLOCK_SYSEX_BYTES: usize = 16 * 1024;

/// One decoded message, as a list keeps it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Entry {
    frame: u32,
    kept: Kept,
}

#[derive(Debug, Clone, Copy)]
enum Kept {
    Message(Message<'static>),
    /// A SysEx, as a range of the list's SysEx bytes.
    SysEx {
        group: Group,
        start: usize,
        len: usize,
    },
}

/// The room that a block's [`Midi`] is decoded into, allocated once at
/// activation so that reading a block allocates nothing.
///
/// [`read`](Self::read) decodes packets in order with the list's own decoder,
/// so state that spans blocks, such as a sounding note or a SysEx under way,
/// stays correct. [`midi`](Self::midi) returns the whole block and
/// [`Midi::between`] part of it. A message or SysEx that doesn't fit is
/// dropped whole and counted in [`Midi::dropped`].
///
/// # Examples
///
/// ```
/// use hapto_midi::{Message, MidiList};
/// use hapto_ump::{Event, EventList};
///
/// let mut packets = EventList::with_capacity(4);
/// packets.push(Event::new(3, [0x4090_3C00, 0xFFFF_0000, 0, 0]));
/// packets.push(Event::new(9, [0x4080_3C00, 0x8000_0000, 0, 0]));
///
/// let mut list = MidiList::new();
/// list.read(&packets, 16);
/// let frames: Vec<usize> = list.midi().iter().map(|(frame, _)| frame).collect();
/// assert_eq!(frames, [3, 9]);
/// // The block's second half, with frames from its start.
/// let later = list.midi().between(8, 16);
/// assert!(matches!(later.iter().next(), Some((1, Message::NoteOff(_)))));
/// ```
pub struct MidiList {
    decoder: Decoder,
    room: Room,
}

/// What a list keeps of a block, in room allocated once.
struct Room {
    entries: Vec<Entry>,
    sysex: Vec<u8>,
    /// The block's packets, at frames within it and in time order.
    packets: Vec<Event>,
    /// The block's losses, as [`Midi::dropped`] counts them.
    dropped: u64,
}

impl fmt::Debug for MidiList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MidiList")
            .field("messages", &self.room.entries.len())
            .field("packets", &self.room.packets.len())
            .field("dropped", &self.room.dropped)
            .finish_non_exhaustive()
    }
}

impl Default for MidiList {
    fn default() -> Self {
        MidiList::new()
    }
}

impl MidiList {
    /// Returns a list with room per block for [`MESSAGES`] messages, as many
    /// packets and [`BLOCK_SYSEX_BYTES`] of SysEx, with a [`Decoder::new`].
    ///
    /// Thread: any but audio, because it allocates.
    pub fn new() -> MidiList {
        MidiList::with_room(MESSAGES, BLOCK_SYSEX_BYTES, Decoder::new())
    }

    /// Returns a list with room per block for `messages` messages, as many
    /// packets and `sysex_bytes` of SysEx, with `decoder`.
    ///
    /// Thread: any but audio, because it allocates.
    pub fn with_room(messages: usize, sysex_bytes: usize, decoder: Decoder) -> MidiList {
        MidiList {
            decoder,
            room: Room {
                entries: Vec::with_capacity(messages),
                sysex: Vec::with_capacity(sysex_bytes),
                packets: Vec::with_capacity(messages),
                dropped: 0,
            },
        }
    }

    /// Decodes `block`, one block's packets in time order, in place of what
    /// the list held.
    ///
    /// Each packet lands at its frame in a block of `frames` frames. To keep
    /// time order, a packet past the block lands at the last frame, and one
    /// earlier than the packet before it lands at that packet's frame. The
    /// list's drops include those of `block`.
    ///
    /// Thread: audio. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn read(&mut self, block: &EventList, frames: usize) {
        let room = &mut self.room;
        room.clear();
        room.dropped = u64::from(block.dropped());
        let decoder_drops = self.decoder.dropped();
        let last = Event::last_offset(frames);
        let mut floor = 0;
        for event in block.as_slice() {
            let frame = event.offset.min(last).max(floor);
            floor = frame;
            if room.packets.len() < room.packets.capacity() {
                room.packets.push(Event {
                    offset: frame,
                    ..*event
                });
            } else {
                room.lose();
            }
            let decoded = self.decoder.read(event);
            room.keep(frame, &self.decoder, decoded);
        }
        let decoded_drops = self.decoder.dropped().saturating_sub(decoder_drops);
        room.dropped = room.dropped.saturating_add(decoded_drops);
    }

    /// Returns the whole block's MIDI.
    pub fn midi(&self) -> Midi<'_> {
        Midi {
            entries: &self.room.entries,
            sysex: &self.room.sysex,
            packets: &self.room.packets,
            start: 0,
            dropped: self.room.dropped,
        }
    }

    /// Forgets every sounding note and any SysEx under way, as at an
    /// activation.
    pub fn reset(&mut self) {
        self.decoder.reset();
        self.room.clear();
    }
}

impl Room {
    fn clear(&mut self) {
        self.entries.clear();
        self.sysex.clear();
        self.packets.clear();
        self.dropped = 0;
    }

    fn lose(&mut self) {
        self.dropped = self.dropped.saturating_add(1);
    }

    /// Keeps a message at `frame`, or counts a drop if the room is full.
    fn push(&mut self, frame: u32, kept: Kept) {
        if self.entries.len() < self.entries.capacity() {
            self.entries.push(Entry { frame, kept });
        } else {
            self.lose();
        }
    }

    /// Keeps what one packet decoded to, copying a SysEx's bytes into the
    /// room.
    fn keep(&mut self, frame: u32, decoder: &Decoder, decoded: Decoded) {
        match decoded {
            Decoded::Nothing => {}
            Decoded::One(message) => self.push(frame, Kept::Message(message)),
            Decoded::Offs(off) => {
                for &id in decoder.ended() {
                    let message = Message::NoteOff(NoteOff { id, ..off });
                    self.push(frame, Kept::Message(message));
                }
            }
            Decoded::SysEx(group) => {
                let data = decoder.sysex_bytes();
                let start = self.sysex.len();
                if data.len() > self.sysex.capacity().saturating_sub(start) {
                    self.lose();
                    return;
                }
                self.sysex.extend_from_slice(data);
                let len = data.len();
                self.push(frame, Kept::SysEx { group, start, len });
            }
        }
    }
}

/// One block's decoded MIDI, the type of [`Block::midi`] in `hapto-host`.
///
/// Like JUCE's `MidiBuffer`, it yields each message with its frame, in order
/// of arrival, and can be read any number of times. [`packets`](Self::packets)
/// keeps the original packets unchanged, for a plugin such as an echo or a
/// recorder.
///
/// [`Block::midi`]: https://docs.rs/hapto-host/latest/hapto_host/struct.Block.html#structfield.midi
///
/// # Examples
///
/// ```
/// use hapto_midi::{Message, Midi, controller};
///
/// /// Tracks the sustain pedal, and ignores other MIDI.
/// fn read(midi: Midi<'_>, sustain: &mut bool) {
///     for (_frame, message) in midi {
///         if let Some(pedal) = controller::SUSTAIN.read(&message) {
///             *sustain = pedal.is_down();
///         }
///     }
/// }
///
/// let mut sustain = false;
/// // A block with no MIDI, as a test makes one.
/// read(Midi::default(), &mut sustain);
/// assert!(!sustain);
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct Midi<'a> {
    entries: &'a [Entry],
    sysex: &'a [u8],
    packets: &'a [Event],
    /// The block's frame that this part starts at.
    start: u32,
    dropped: u64,
}

impl<'a> Midi<'a> {
    /// Returns each message with its frame, in the order of arrival.
    pub fn iter(&self) -> Iter<'a> {
        Iter {
            entries: self.entries.iter(),
            sysex: self.sysex,
            start: self.start,
        }
    }

    /// Returns the original packets, unchanged, each at its frame.
    pub fn packets(&self) -> Packets<'a> {
        Packets {
            packets: self.packets.iter(),
            start: self.start,
        }
    }

    /// Returns the messages without frames, for a part whose messages all
    /// arrive at its first frame, such as a span's.
    pub fn messages(&self) -> impl ExactSizeIterator<Item = Message<'a>> + use<'a> {
        self.iter().map(|(_, message)| message)
    }

    /// Returns the part at frames `from` up to `to`, renumbered from `from`.
    ///
    /// Only a part from frame 0 reports drops.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_midi::MidiList;
    /// use hapto_ump::{Event, EventList};
    ///
    /// let mut packets = EventList::with_capacity(2);
    /// packets.push(Event::new(2, [0x40D0_0000, 0, 0, 0]));
    /// packets.push(Event::new(6, [0x40D0_0000, u32::MAX, 0, 0]));
    /// let mut list = MidiList::new();
    /// list.read(&packets, 8);
    /// let later = list.midi().between(4, 8);
    /// assert_eq!((later.len(), later.first_frame()), (1, Some(2)));
    /// ```
    pub fn between(&self, from: usize, to: usize) -> Midi<'a> {
        let base = u64::from(self.start);
        let at =
            |frame: usize| u32::try_from(base.saturating_add(frame as u64)).unwrap_or(u32::MAX);
        let (from, to) = (at(from), at(to));
        let first = self.entries.partition_point(|entry| entry.frame < from);
        let end = self.entries.partition_point(|entry| entry.frame < to);
        let first_packet = self.packets.partition_point(|packet| packet.offset < from);
        let end_packet = self.packets.partition_point(|packet| packet.offset < to);
        Midi {
            entries: self.entries.get(first..end).unwrap_or(&[]),
            sysex: self.sysex,
            packets: self.packets.get(first_packet..end_packet).unwrap_or(&[]),
            start: from,
            dropped: if from == self.start { self.dropped } else { 0 },
        }
    }

    /// Returns the first message's frame, if there is one.
    pub fn first_frame(&self) -> Option<usize> {
        self.iter().next().map(|(frame, _)| frame)
    }

    /// Returns how many messages arrived.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether no messages arrived.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns how many messages the block lost.
    ///
    /// - Packets that didn't fit in the list's room.
    /// - Messages and whole SysEx that didn't fit in the decoded room.
    /// - SysEx that the [`Decoder`] dropped.
    /// - Notes that lost their place in the decoder's full table, so their
    ///   note-offs no longer name them.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

impl<'a> IntoIterator for Midi<'a> {
    type Item = (usize, Message<'a>);
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

impl<'a> IntoIterator for &Midi<'a> {
    type Item = (usize, Message<'a>);
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Iter<'a> {
        self.iter()
    }
}

/// The messages of a [`Midi`] with their frames, from [`Midi::iter`].
///
/// # Examples
///
/// ```
/// use hapto_midi::{Message, MidiList};
/// use hapto_ump::{Event, EventList};
///
/// let mut packets = EventList::with_capacity(2);
/// packets.push(Event::new(5, [0x40E0_0000, 0xC000_0000, 0, 0]));
/// let mut list = MidiList::new();
/// list.read(&packets, 8);
/// let mut messages = list.midi().iter();
/// assert_eq!(messages.len(), 1);
/// let (frame, message) = messages.next().expect("a bend");
/// assert!(frame == 5 && matches!(message, Message::Bend { .. }));
/// ```
#[derive(Debug, Clone)]
pub struct Iter<'a> {
    entries: std::slice::Iter<'a, Entry>,
    sysex: &'a [u8],
    start: u32,
}

impl<'a> Iterator for Iter<'a> {
    type Item = (usize, Message<'a>);

    fn next(&mut self) -> Option<(usize, Message<'a>)> {
        let entry = self.entries.next()?;
        let frame = entry.frame.saturating_sub(self.start) as usize;
        let message = match entry.kept {
            Kept::Message(message) => message,
            Kept::SysEx { group, start, len } => Message::SysEx {
                group,
                data: self
                    .sysex
                    .get(start..start.saturating_add(len))
                    .unwrap_or(&[]),
            },
        };
        Some((frame, message))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.entries.size_hint()
    }
}

impl ExactSizeIterator for Iter<'_> {}

/// The packets of a [`Midi`], each at its frame, from [`Midi::packets`].
///
/// # Examples
///
/// ```
/// use hapto_midi::{Midi, MidiOut};
/// use hapto_ump::EventList;
///
/// // An echo sends every packet on unchanged.
/// let mut sent = EventList::with_capacity(8);
/// MidiOut::new(&mut sent).extend(Midi::default().packets());
/// assert!(sent.as_slice().is_empty());
/// ```
#[derive(Debug, Clone)]
pub struct Packets<'a> {
    packets: std::slice::Iter<'a, Event>,
    start: u32,
}

impl Iterator for Packets<'_> {
    type Item = Event;

    fn next(&mut self) -> Option<Event> {
        let packet = self.packets.next()?;
        Some(Event {
            offset: packet.offset.saturating_sub(self.start),
            ..*packet
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.packets.size_hint()
    }
}

impl ExactSizeIterator for Packets<'_> {}

/// The MIDI that a block sends, through [`Block::midi_out`] in `hapto-host`.
///
/// Messages go out as MIDI 2.0, downgraded only where the other end needs
/// MIDI 1.0. Messages and packets share one list, in the order that the plugin
/// sends them, which should be time order. What doesn't fit is dropped and
/// counted.
///
/// [`Block::midi_out`]: https://docs.rs/hapto-host/latest/hapto_host/struct.Block.html#structfield.midi_out
///
/// # Examples
///
/// ```
/// use hapto_midi::{Key, MidiOut, NoteOn};
/// use hapto_ump::EventList;
///
/// let mut sent = EventList::with_capacity(4);
/// let mut out = MidiOut::new(&mut sent);
/// // G4, a quarter of the way into a 64-frame block.
/// out.send(16, NoteOn::new(Key::new(67), 1.0));
/// assert_eq!(sent.as_slice()[0].offset, 16);
/// assert_eq!(sent.as_slice()[0].words[0], 0x4090_4300);
/// ```
#[derive(Debug)]
pub struct MidiOut<'a> {
    packets: &'a mut EventList,
}

impl<'a> MidiOut<'a> {
    /// Returns a sender into `packets`.
    pub fn new(packets: &'a mut EventList) -> Self {
        MidiOut { packets }
    }

    /// Encodes `message` and sends it at `frame`.
    ///
    /// The message can be the plugin's own, a note moved with
    /// [`NoteOn::with_key`](crate::NoteOn::with_key), or a message that
    /// arrived, passed on as MIDI thru, which is never implicit.
    ///
    /// A note keeps its attribute and id, so a note that its host named goes
    /// out under the host's id, in the packet's origin, where the host can
    /// match it. A note sent beside it takes an id from
    /// [`NoteId::own`](crate::NoteId::own). Values go out at `f32` precision,
    /// so for the exact packets, extend with [`Midi::packets`]. A SysEx goes
    /// out whole or not at all, and one that doesn't fit is counted as dropped
    /// packets.
    ///
    /// Thread: audio. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn send<'m>(&mut self, frame: usize, message: impl Into<Message<'m>>) {
        let message = message.into();
        if let Message::SysEx { data, .. } = message {
            let needed = SysEx7::count(data.len());
            let room = self
                .packets
                .capacity()
                .saturating_sub(self.packets.as_slice().len());
            if needed > room {
                self.packets
                    .add_dropped(u32::try_from(needed).unwrap_or(u32::MAX));
                return;
            }
        }
        let offset = u32::try_from(frame).unwrap_or(u32::MAX);
        let origin = named(&message);
        let packets = &mut *self.packets;
        encode(&message, |words| {
            packets.push(Event {
                origin,
                ..Event::new(offset, words)
            });
        });
    }

    /// Sends `packet` unchanged at its own frame, for a plugin that keeps or
    /// builds its own packets.
    pub fn send_packet(&mut self, packet: Event) {
        self.packets.push(packet);
    }

    /// Returns how many packets found no room in the block.
    pub fn dropped(&self) -> u32 {
        self.packets.dropped()
    }
}

/// Returns the origin for a message's packets, with the host's note id if the
/// decoder found that the host named the note.
fn named(message: &Message<'_>) -> Origin {
    message
        .note()
        .and_then(|note| note.id)
        .filter(|id| id.is_hosts())
        .map_or(Origin::Midi2, |id| Origin::Note { id: id.get() })
}

/// Returns the packets that a [`MidiOut`] sends for `messages`, each at its
/// frame.
///
/// A test or tool can hand them to a host, a test host or a device instead of
/// hand-written packets. It allocates, so it doesn't belong on the audio
/// thread.
///
/// # Examples
///
/// ```
/// use hapto_midi::controller::VOLUME;
/// use hapto_midi::{Channel, Group, Key, NoteOn, packets_of};
///
/// let packets = packets_of([
///     (0, NoteOn::new(Key::A4, 1.0).into()),
///     (64, VOLUME.message(Group::FIRST, Channel::FIRST, 0.5)),
/// ]);
/// assert_eq!(packets.len(), 2);
/// assert_eq!(packets[0].words[0], 0x4090_4500, "a MIDI 2.0 note-on of A4");
/// assert_eq!(packets[1].offset, 64);
/// ```
pub fn packets_of<'m>(messages: impl IntoIterator<Item = (usize, Message<'m>)>) -> Vec<Event> {
    let messages: Vec<(usize, Message<'m>)> = messages.into_iter().collect();
    // A SysEx takes a packet per six bytes, and any other message takes one
    // or two.
    let room = messages
        .iter()
        .map(|(_, message)| match message {
            Message::SysEx { data, .. } => SysEx7::count(data.len()),
            _ => 2,
        })
        .fold(0_usize, usize::saturating_add);
    let mut packets = EventList::with_capacity(room);
    MidiOut::new(&mut packets).extend(messages);
    packets.as_slice().to_vec()
}

/// Sends each message at its frame, as [`MidiOut::send`] does. MIDI thru is
/// `midi_out.extend(midi)`.
///
/// # Examples
///
/// ```
/// use hapto_midi::{MidiList, MidiOut};
/// use hapto_ump::{Event, EventList};
///
/// let mut packets = EventList::with_capacity(4);
/// packets.push(Event::new(3, [0x4090_3C00, 0xFFFF_0000, 0, 0]));
/// let mut list = MidiList::new();
/// list.read(&packets, 16);
///
/// // Everything that arrived, sent on.
/// let mut sent = EventList::with_capacity(4);
/// MidiOut::new(&mut sent).extend(list.midi());
/// assert_eq!(sent.as_slice()[0].offset, 3);
/// ```
impl<'m> Extend<(usize, Message<'m>)> for MidiOut<'_> {
    fn extend<I: IntoIterator<Item = (usize, Message<'m>)>>(&mut self, messages: I) {
        for (frame, message) in messages {
            self.send(frame, message);
        }
    }
}

/// Sends each packet unchanged, as [`MidiOut::send_packet`] does.
impl Extend<Event> for MidiOut<'_> {
    fn extend<I: IntoIterator<Item = Event>>(&mut self, packets: I) {
        for packet in packets {
            self.send_packet(packet);
        }
    }
}

/// Sends each packet unchanged, as [`MidiOut::send_packet`] does.
impl<'e> Extend<&'e Event> for MidiOut<'_> {
    fn extend<I: IntoIterator<Item = &'e Event>>(&mut self, packets: I) {
        for &packet in packets {
            self.send_packet(packet);
        }
    }
}
