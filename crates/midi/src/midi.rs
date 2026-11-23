//! A block's MIDI: its packets decoded once into messages, each at its
//! frame, read as often as needed; and the MIDI a block sends.

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

use hapto_ump::{Event, EventList, Origin};

use crate::decoder::{Decoded, Decoder};
use crate::encode::encode;
use crate::message::{Message, NoteOff};
use crate::values::Group;

/// How many messages a list made with [`MidiList::new`] holds in a block.
pub const MESSAGES: usize = 2048;
/// How many SysEx bytes, across a block's SysEx messages, it holds.
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
    /// A SysEx's bytes, in the list's own room.
    SysEx {
        group: Group,
        start: usize,
        len: usize,
    },
}

/// A block's MIDI, decoded: the room a block's [`Midi`] is read from, made
/// once, at activation, so reading a block allocates nothing.
///
/// [`read`](Self::read) decodes a block's packets in order, with a decoder
/// of its own, so state that spans blocks, a note sounding or a SysEx under
/// way, stays right. [`midi`](Self::midi) is the whole block's MIDI, and
/// [`between`](Self::between) part of it. A message or SysEx byte past its
/// room is dropped and counted in [`Midi::dropped`].
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
/// // The second half of the block, its frames from its own start.
/// let later = list.between(8, 16);
/// assert!(matches!(later.iter().next(), Some((1, Message::NoteOff(_)))));
/// ```
pub struct MidiList {
    decoder: Decoder,
    entries: Vec<Entry>,
    sysex: Vec<u8>,
    /// The block's packets, at frames of the block and in time order.
    packets: Vec<Event>,
    /// What the block lost.
    dropped: u64,
}

impl fmt::Debug for MidiList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MidiList")
            .field("messages", &self.entries.len())
            .field("packets", &self.packets.len())
            .field("dropped", &self.dropped)
            .finish_non_exhaustive()
    }
}

impl Default for MidiList {
    fn default() -> Self {
        MidiList::new()
    }
}

impl MidiList {
    /// A list with room for [`MESSAGES`] messages, as many packets, and
    /// [`BLOCK_SYSEX_BYTES`] of SysEx a block, over a [`Decoder::new`].
    ///
    /// Thread: any but audio: it allocates its room.
    pub fn new() -> MidiList {
        MidiList::with_room(MESSAGES, BLOCK_SYSEX_BYTES, Decoder::new())
    }

    /// A list with room for `messages` messages and as many packets, and
    /// `sysex_bytes` of SysEx a block, over `decoder`.
    ///
    /// Thread: any but audio: it allocates its room.
    pub fn with_room(messages: usize, sysex_bytes: usize, decoder: Decoder) -> MidiList {
        MidiList {
            decoder,
            entries: Vec::with_capacity(messages),
            sysex: Vec::with_capacity(sysex_bytes),
            packets: Vec::with_capacity(messages),
            dropped: 0,
        }
    }

    /// Decodes `block`, one block's packets in time order, replacing what
    /// the list held. A packet lands at its frame of a block of `frames`
    /// frames: one past the block at its last frame, and one before the
    /// packet before it at that one's frame, so the list stays in time
    /// order. The block's own drops count as the list's.
    ///
    /// Thread: audio. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn read(&mut self, block: &EventList, frames: usize) {
        let MidiList {
            decoder,
            entries,
            sysex,
            packets,
            dropped,
        } = self;
        entries.clear();
        sysex.clear();
        packets.clear();
        *dropped = u64::from(block.dropped());
        let decoder_drops = decoder.dropped();
        let last = u32::try_from(frames.saturating_sub(1)).unwrap_or(u32::MAX);
        let mut floor = 0;
        for event in block.as_slice() {
            let frame = event.offset.min(last).max(floor);
            floor = frame;
            if packets.len() < packets.capacity() {
                packets.push(Event {
                    offset: frame,
                    ..*event
                });
            } else {
                *dropped = dropped.saturating_add(1);
            }
            let decoded = decoder.read(event);
            keep(entries, sysex, dropped, frame, decoder, decoded);
        }
        *dropped = dropped.saturating_add(decoder.dropped().saturating_sub(decoder_drops));
    }

    /// The whole block's MIDI.
    pub fn midi(&self) -> Midi<'_> {
        Midi {
            entries: &self.entries,
            sysex: &self.sysex,
            packets: &self.packets,
            start: 0,
            dropped: self.dropped,
        }
    }

    /// The MIDI at frames `from` up to `to`, at frames from `from`. The
    /// block's drops come with the part that starts at frame 0.
    pub fn between(&self, from: usize, to: usize) -> Midi<'_> {
        self.midi().between(from, to)
    }

    /// Forgets every note sounding and any SysEx under way, as at an
    /// activation.
    pub fn reset(&mut self) {
        self.decoder.reset();
        self.entries.clear();
        self.sysex.clear();
        self.packets.clear();
        self.dropped = 0;
    }
}

/// Keeps what one packet decoded to, a SysEx's bytes copied into the
/// list's room.
fn keep(
    entries: &mut Vec<Entry>,
    sysex: &mut Vec<u8>,
    dropped: &mut u64,
    frame: u32,
    decoder: &Decoder,
    decoded: Decoded,
) {
    let mut push = |kept: Kept, entries: &mut Vec<Entry>| {
        if entries.len() < entries.capacity() {
            entries.push(Entry { frame, kept });
        } else {
            *dropped = dropped.saturating_add(1);
        }
    };
    match decoded {
        Decoded::Nothing => {}
        Decoded::One(message) => push(Kept::Message(message), entries),
        Decoded::Offs(off) => {
            for &id in decoder.ended() {
                push(
                    Kept::Message(Message::NoteOff(NoteOff { id, ..off })),
                    entries,
                );
            }
        }
        Decoded::SysEx(group) => {
            let data = decoder.sysex_bytes();
            let start = sysex.len();
            if data.len() > sysex.capacity().saturating_sub(start) {
                push_dropped(dropped);
                return;
            }
            sysex.extend_from_slice(data);
            push(
                Kept::SysEx {
                    group,
                    start,
                    len: data.len(),
                },
                entries,
            );
        }
    }
}

fn push_dropped(dropped: &mut u64) {
    *dropped = dropped.saturating_add(1);
}

/// One block's MIDI, decoded once: [`Block::midi`] in `hapto-host`. It
/// reads like JUCE's `MidiBuffer`, each message with its frame, in the
/// order it came, as often as needed.
///
/// The packets the messages came in are kept beside them, in
/// [`packets`](Self::packets), exactly as they came, for a plugin that
/// wants them so: an echo, or a recorder.
///
/// [`Block::midi`]: https://docs.rs/hapto-host/latest/hapto_host/struct.Block.html#structfield.midi
///
/// # Examples
///
/// ```
/// use hapto_midi::{Message, Midi, controller};
///
/// /// What a plugin keeps of its MIDI: the sustain pedal.
/// fn read(midi: Midi<'_>, sustain: &mut bool) {
///     for (_frame, message) in midi {
///         if let Message::Control { controller: controller::SUSTAIN, value, .. } = message {
///             *sustain = value >= 0.5;
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
    /// The frame of the block this part starts at.
    start: u32,
    dropped: u64,
}

impl<'a> Midi<'a> {
    /// Each message with its frame, in the order they came.
    pub fn iter(&self) -> Iter<'a> {
        Iter {
            entries: self.entries.iter(),
            sysex: self.sysex,
            start: self.start,
        }
    }

    /// The packets the messages were decoded from, exactly as they came,
    /// each at its frame.
    pub fn packets(&self) -> Packets<'a> {
        Packets {
            packets: self.packets.iter(),
            start: self.start,
        }
    }

    /// The messages alone, without their frames: for a part of a block in
    /// which they all arrive at its first frame, a span's.
    pub fn messages(&self) -> impl ExactSizeIterator<Item = Message<'a>> + use<'a> {
        self.iter().map(|(_, message)| message)
    }

    /// The part of it at frames `from` up to `to`, at frames from `from`.
    /// Its drops come with the part that starts at frame 0.
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

    /// The frame its first message arrives at, if any does.
    pub fn first_frame(&self) -> Option<usize> {
        self.iter().next().map(|(frame, _)| frame)
    }

    /// How many messages arrived.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether none did.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many messages the block lost: packets past its list's room,
    /// messages and SysEx bytes past the decoded room, notes past the
    /// decoder's table, and SysEx messages cut short.
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

/// Each message of a [`Midi`] with its frame: what [`Midi::iter`] returns.
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

/// The packets of a [`Midi`], each at its frame: what [`Midi::packets`]
/// returns.
///
/// # Examples
///
/// ```
/// use hapto_midi::{Midi, MidiOut};
/// use hapto_ump::EventList;
///
/// // An echo: every packet out as it came.
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

/// The MIDI a block sends: [`Block::midi_out`] in `hapto-host`. Messages
/// go out as MIDI 2.0, and are downgraded on the way out only where the
/// other end needs MIDI 1.0. Messages and packets go out in one list, in the
/// order the plugin sends them, which should be time order. One past the
/// list's room is dropped and counted.
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
/// // A note a fifth up, a quarter of a block of 64 frames in.
/// out.send(16, NoteOn::new(Key::new(67), 1.0));
/// assert_eq!(sent.as_slice()[0].offset, 16);
/// assert_eq!(sent.as_slice()[0].words[0], 0x4090_4300);
/// ```
#[derive(Debug)]
pub struct MidiOut<'a> {
    packets: &'a mut EventList,
}

impl<'a> MidiOut<'a> {
    /// Sends into `packets`.
    pub fn new(packets: &'a mut EventList) -> Self {
        MidiOut { packets }
    }

    /// Sends `message` at `frame`, encoded anew: a transposed note, say.
    ///
    /// Thread: audio. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn send<'m>(&mut self, frame: usize, message: impl Into<Message<'m>>) {
        self.forward(frame, &message.into());
    }

    /// Sends a message that arrived on to the next plugin, at `frame`: MIDI
    /// thru, which is never implicit. A note keeps its attribute, a note its
    /// host named keeps the host's id, in the packet's origin, so the host
    /// knows it again, and a value goes at an `f32`'s precision: for the
    /// packets exactly as they came, extend with [`Midi::packets`].
    ///
    /// Thread: audio. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn forward(&mut self, frame: usize, message: &Message<'_>) {
        let offset = u32::try_from(frame).unwrap_or(u32::MAX);
        let origin = named(message);
        let packets = &mut *self.packets;
        encode(message, |words| {
            packets.push(Event {
                origin,
                ..Event::new(offset, words)
            });
        });
    }

    /// Sends `packet` as it is, at its own frame: for a plugin that keeps
    /// or builds packets itself.
    pub fn send_packet(&mut self, packet: Event) {
        self.packets.push(packet);
    }

    /// How many packets found no room in the block.
    pub fn dropped(&self) -> u32 {
        self.packets.dropped()
    }
}

/// Where a message's packets come from: the host's note id, for a note or
/// a note's own message its host named, as the decoder found it.
fn named(message: &Message<'_>) -> Origin {
    let id = match message {
        Message::NoteOn(on) => Some(on.id),
        Message::NoteOff(off) => Some(off.id),
        Message::NotePitch { note, .. }
        | Message::NoteBend { note, .. }
        | Message::NotePressure { note, .. }
        | Message::NoteControl { note, .. }
        | Message::NoteAssignable { note, .. }
        | Message::NoteManagement { note, .. } => note.id,
        _ => None,
    };
    id.filter(|id| id.is_hosts())
        .map_or(Origin::Midi2, |id| Origin::Note { id: id.get() })
}

/// Sends each packet as it is, as [`MidiOut::send_packet`] does.
impl Extend<Event> for MidiOut<'_> {
    fn extend<I: IntoIterator<Item = Event>>(&mut self, packets: I) {
        for packet in packets {
            self.send_packet(packet);
        }
    }
}

/// Sends each packet as it is, as [`MidiOut::send_packet`] does.
impl<'e> Extend<&'e Event> for MidiOut<'_> {
    fn extend<I: IntoIterator<Item = &'e Event>>(&mut self, packets: I) {
        for &packet in packets {
            self.send_packet(packet);
        }
    }
}
