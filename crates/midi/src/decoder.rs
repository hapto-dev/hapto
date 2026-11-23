//! From packets to messages: what the engine runs over every block, and a
//! plugin may run on its main thread or in a test.

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

use hapto_ump::scale::down_32_to_14;
use hapto_ump::{Attribute, ChannelMessage, ChannelVoice, Event};

use crate::message::{Message, NoteKey, NoteOff, NoteOn};
use crate::values::{Centred, Channel, Group, Key, NoteId, Pitch, Program};

/// How many notes a decoder made with [`Decoder::new`] keeps track of at
/// once, across every group and channel.
pub const NOTES: usize = 256;
/// The longest SysEx message a decoder made with [`Decoder::new`] joins.
pub const SYSEX_BYTES: usize = 4096;

/// A note sounding, as the decoder keeps it.
#[derive(Debug, Clone, Copy)]
struct Sounding {
    group: Group,
    channel: Channel,
    key: Key,
    id: NoteId,
    /// Per-note management detached it: it takes no more per-note
    /// messages.
    detached: bool,
}

/// Reads packets into [`Message`]s, as the engine reads each block's.
///
/// It reads MIDI 2.0 alone, which is all that reaches a plugin: MIDI 1.0 is
/// upgraded before it. It keeps what spans packets in tables made with it,
/// so decoding allocates nothing:
///
/// - The notes sounding, so each note-on gets an id, unless its host gave
///   one, and a note-off ends every note on its key. A note-off for a key
///   with no note it knows of still comes, under an id no note has, so a
///   plugin that ends notes by key hears it. A note past the table sounds,
///   and is counted in [`dropped`](Self::dropped).
/// - A SysEx message as its packets arrive, which comes whole once its last
///   does. One past the buffer, one cut short, or one begun on another group
///   while one is under way is dropped and counted.
///
/// # Examples
///
/// ```
/// use hapto_midi::{Decoder, Key, Message};
/// use hapto_ump::Event;
///
/// let mut decoder = Decoder::new();
/// let on = |key: u32| Event::new(0, [0x4090_0000 | key << 8, 0xFFFF_0000, 0, 0]);
/// let off = Event::new(9, [0x4080_3C00, 0x8000_0000, 0, 0]);
///
/// // The same key twice is two notes, and one note-off ends both.
/// decoder.decode(&on(60)).count();
/// decoder.decode(&on(60)).count();
/// let ended: Vec<Message<'_>> = decoder.decode(&off).collect();
/// assert_eq!(ended.len(), 2);
/// assert!(ended.iter().all(|m| matches!(m, Message::NoteOff(off) if off.key == Key::C4)));
/// ```
pub struct Decoder {
    /// The notes sounding, oldest first: the first `sounding` slots.
    notes: Box<[Sounding]>,
    sounding: usize,
    /// How many ids it has made up.
    made: u32,
    /// The notes the last note-off ended: the first `ended` slots.
    ends: Box<[NoteId]>,
    ended: usize,
    /// The SysEx under way: its bytes so far, and its group.
    sysex: Box<[u8]>,
    sysex_len: usize,
    sysex_group: Option<Group>,
    /// Whether the SysEx under way outgrew the buffer.
    sysex_over: bool,
    dropped: u64,
}

impl fmt::Debug for Decoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Decoder")
            .field("sounding", &self.sounding)
            .field("sysex_under_way", &self.sysex_group.is_some())
            .field("dropped", &self.dropped)
            .finish_non_exhaustive()
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Decoder::new()
    }
}

/// What one packet decoded to, before it is handed out as messages.
#[derive(Debug, Clone, Copy)]
enum Decoded {
    Nothing,
    One(Message<'static>),
    /// A note-off for each note in the decoder's `ends`.
    Offs(NoteOff),
    /// The SysEx in the decoder's buffer, now whole.
    SysEx(Group),
}

/// The messages one packet decoded to, in order: what
/// [`Decoder::decode`] returns.
///
/// # Examples
///
/// ```
/// use hapto_midi::{Decoder, Message};
/// use hapto_ump::Event;
///
/// let mut decoder = Decoder::new();
/// // A SysEx's first part completes nothing yet.
/// let start = Event::new(0, [0x3016_7E7F, 0x0601_0203, 0, 0]);
/// assert_eq!(decoder.decode(&start).count(), 0);
/// let end = Event::new(0, [0x3031_0400, 0, 0, 0]);
/// for message in decoder.decode(&end) {
///     assert!(matches!(message, Message::SysEx { data, .. } if data.len() == 7));
/// }
/// ```
#[derive(Debug)]
pub struct Messages<'d> {
    decoder: &'d Decoder,
    decoded: Decoded,
    next: usize,
}

impl<'d> Iterator for Messages<'d> {
    type Item = Message<'d>;

    fn next(&mut self) -> Option<Message<'d>> {
        let at = self.next;
        self.next = at.saturating_add(1);
        match self.decoded {
            Decoded::Nothing => None,
            Decoded::One(message) => (at == 0).then_some(message),
            Decoded::Offs(off) => {
                let id = *self.decoder.ends.get(..self.decoder.ended)?.get(at)?;
                Some(Message::NoteOff(NoteOff { id, ..off }))
            }
            Decoded::SysEx(group) => (at == 0).then(|| Message::SysEx {
                group,
                data: self
                    .decoder
                    .sysex
                    .get(..self.decoder.sysex_len)
                    .unwrap_or(&[]),
            }),
        }
    }
}

impl Decoder {
    /// A decoder that keeps track of up to [`NOTES`] notes and joins SysEx
    /// of up to [`SYSEX_BYTES`].
    ///
    /// Thread: any but audio: it allocates its tables.
    pub fn new() -> Decoder {
        Decoder::with_room(NOTES, SYSEX_BYTES)
    }

    /// A decoder that keeps track of up to `notes` notes and joins SysEx of
    /// up to `sysex_bytes`.
    ///
    /// Thread: any but audio: it allocates its tables.
    pub fn with_room(notes: usize, sysex_bytes: usize) -> Decoder {
        let unset = Sounding {
            group: Group::FIRST,
            channel: Channel::FIRST,
            key: Key::C4,
            id: NoteId::made_up(0),
            detached: false,
        };
        Decoder {
            notes: vec![unset; notes].into_boxed_slice(),
            sounding: 0,
            made: 0,
            ends: vec![NoteId::made_up(0); notes.max(1)].into_boxed_slice(),
            ended: 0,
            sysex: vec![0; sysex_bytes].into_boxed_slice(),
            sysex_len: 0,
            sysex_group: None,
            sysex_over: false,
            dropped: 0,
        }
    }

    /// The messages `event` completes: none for a packet Hapto does not
    /// read or the first parts of a SysEx, several for a note-off that ends
    /// several notes.
    ///
    /// Thread: any. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn decode(&mut self, event: &Event) -> Messages<'_> {
        let decoded = self.read(event);
        Messages {
            decoder: self,
            decoded,
            next: 0,
        }
    }

    /// How many notes found no room in the table, and how many SysEx
    /// messages were dropped, since it was made.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// How many notes sound now, as far as it knows.
    pub fn sounding(&self) -> usize {
        self.sounding
    }

    /// Forgets every note sounding and any SysEx under way, as at an
    /// activation.
    pub fn reset(&mut self) {
        self.sounding = 0;
        self.ended = 0;
        self.sysex_len = 0;
        self.sysex_group = None;
        self.sysex_over = false;
    }

    fn read(&mut self, event: &Event) -> Decoded {
        let head = event.words[0];
        match head >> 28 {
            // Real time may fall inside a SysEx, and so may the groupless
            // utility and stream messages.
            0x0 | 0xF => Decoded::Nothing,
            0x1 if (head >> 16) & 0xFF >= 0xF8 => Decoded::Nothing,
            0x3 => self.sysex(event.words),
            _ => {
                // Anything else on a SysEx's group ends it, cut short.
                if self.sysex_group == Some(Group::field(event.group())) {
                    self.drop_sysex();
                }
                match ChannelVoice::decode(event.words) {
                    Some(voice) => self.voice(voice),
                    None => Decoded::Nothing,
                }
            }
        }
    }

    fn voice(&mut self, voice: ChannelVoice) -> Decoded {
        let group = Group::field(voice.group);
        let channel = Channel::field(voice.channel);
        if let Some(message) = self.per_note(group, channel, voice.message) {
            return Decoded::One(message);
        }
        let message = match voice.message {
            ChannelMessage::NoteOn {
                note,
                velocity,
                attribute,
            } => Message::NoteOn(self.note_on(group, channel, Key::new(note), velocity, attribute)),
            ChannelMessage::NoteOff { note, velocity, .. } => {
                return self.note_off(group, channel, Key::new(note), velocity);
            }
            ChannelMessage::ControlChange { controller, value } => Message::Control {
                group,
                channel,
                controller,
                value: unit(value),
            },
            ChannelMessage::ProgramChange { program, bank } => Message::Program {
                group,
                channel,
                program: Program::from_wire(program),
                bank: bank.map(|bank| u16::from(bank.msb & 0x7F) << 7 | u16::from(bank.lsb & 0x7F)),
            },
            ChannelMessage::ChannelPressure { value } => Message::Pressure {
                group,
                channel,
                value: unit(value),
            },
            ChannelMessage::PitchBend { value } => Message::Bend {
                group,
                channel,
                value: Centred::from_bits(value),
            },
            ChannelMessage::RegisteredController {
                bank: 0,
                index: BEND_RANGE,
                value,
            } => Message::BendRange {
                group,
                channel,
                semitones: semitones(value),
            },
            ChannelMessage::RegisteredController { bank, index, value } => Message::Registered {
                group,
                channel,
                bank,
                index,
                value: unit(value),
            },
            ChannelMessage::AssignableController { bank, index, value } => Message::Assignable {
                group,
                channel,
                bank,
                index,
                value: unit(value),
            },
            _ => return Decoded::Nothing,
        };
        Decoded::One(message)
    }

    /// A note's own message, named by its key; `None` for any other.
    fn per_note(
        &mut self,
        group: Group,
        channel: Channel,
        message: ChannelMessage,
    ) -> Option<Message<'static>> {
        Some(match message {
            ChannelMessage::PolyPressure { note, value } => Message::NotePressure {
                note: self.key(group, channel, note),
                value: unit(value),
            },
            ChannelMessage::RegisteredPerNoteController { note, index, value } => {
                let note = self.key(group, channel, note);
                if index == PER_NOTE_PITCH {
                    Message::NotePitch {
                        note,
                        pitch: pitch_7_25(value),
                    }
                } else {
                    Message::NoteControl {
                        note,
                        controller: index,
                        value: unit(value),
                    }
                }
            }
            ChannelMessage::AssignablePerNoteController { note, index, value } => {
                Message::NoteAssignable {
                    note: self.key(group, channel, note),
                    index,
                    value: unit(value),
                }
            }
            ChannelMessage::PerNotePitchBend { note, value } => Message::NoteBend {
                note: self.key(group, channel, note),
                value: Centred::from_bits(value),
            },
            ChannelMessage::PerNoteManagement {
                note,
                detach,
                reset,
            } => {
                let named = self.key(group, channel, note);
                if detach {
                    self.detach(group, channel, Key::new(note));
                }
                Message::NoteManagement {
                    note: named,
                    detach,
                    reset,
                }
            }
            _ => return None,
        })
    }

    fn note_on(
        &mut self,
        group: Group,
        channel: Channel,
        key: Key,
        velocity: u16,
        attribute: Attribute,
    ) -> NoteOn {
        let id = self.make_id();
        match self.notes.get_mut(self.sounding) {
            Some(slot) => {
                *slot = Sounding {
                    group,
                    channel,
                    key,
                    id,
                    detached: false,
                };
                self.sounding = self.sounding.saturating_add(1);
            }
            None => self.dropped = self.dropped.saturating_add(1),
        }
        NoteOn {
            id,
            group,
            channel,
            key,
            velocity: f32::from(velocity) / f32::from(u16::MAX),
            attribute,
        }
    }

    /// Ends every note on the key, moving their ids to `ends`.
    fn note_off(&mut self, group: Group, channel: Channel, key: Key, velocity: u16) -> Decoded {
        self.ended = 0;
        let mut kept = 0;
        for index in 0..self.sounding {
            let Some(&note) = self.notes.get(index) else {
                break;
            };
            if (note.group, note.channel, note.key) == (group, channel, key) {
                if let Some(end) = self.ends.get_mut(self.ended) {
                    *end = note.id;
                    self.ended = self.ended.saturating_add(1);
                }
            } else {
                if let Some(slot) = self.notes.get_mut(kept) {
                    *slot = note;
                }
                kept = kept.saturating_add(1);
            }
        }
        self.sounding = kept;
        if self.ended == 0 {
            // No note it knows of: an id no note has.
            let id = self.make_id();
            if let Some(end) = self.ends.get_mut(0) {
                *end = id;
                self.ended = 1;
            }
        }
        Decoded::Offs(NoteOff {
            id: NoteId::made_up(0),
            group,
            channel,
            key,
            velocity: f32::from(velocity) / f32::from(u16::MAX),
        })
    }

    /// The key a per-note message names, with the newest note on it that
    /// takes per-note messages.
    fn key(&self, group: Group, channel: Channel, key: u8) -> NoteKey {
        let key = Key::new(key);
        let id = self
            .notes
            .get(..self.sounding)
            .unwrap_or(&[])
            .iter()
            .rev()
            .find(|note| (note.group, note.channel, note.key) == (group, channel, key))
            .filter(|note| !note.detached)
            .map(|note| note.id);
        NoteKey {
            group,
            channel,
            key,
            id,
        }
    }

    fn detach(&mut self, group: Group, channel: Channel, key: Key) {
        let sounding = self.sounding;
        for note in self.notes.iter_mut().take(sounding) {
            if (note.group, note.channel, note.key) == (group, channel, key) {
                note.detached = true;
            }
        }
    }

    fn make_id(&mut self) -> NoteId {
        let id = NoteId::made_up(self.made & 0x7FFF_FFFF);
        self.made = self.made.wrapping_add(1);
        id
    }

    /// One 7-bit SysEx packet.
    fn sysex(&mut self, words: [u32; 4]) -> Decoded {
        let head = words[0];
        let group = Group::field(((head >> 24) & 0xF) as u8);
        let count = (((head >> 16) & 0xF) as usize).min(6);
        let payload = [
            (head >> 8) as u8,
            head as u8,
            (words[1] >> 24) as u8,
            (words[1] >> 16) as u8,
            (words[1] >> 8) as u8,
            words[1] as u8,
        ];
        let bytes = payload.get(..count).unwrap_or(&[]);
        let status = (head >> 20) & 0xF;
        match status {
            COMPLETE | START => {
                if let Some(under_way) = self.sysex_group {
                    if under_way != group {
                        // One under way on another group keeps the buffer.
                        self.dropped = self.dropped.saturating_add(1);
                        return Decoded::Nothing;
                    }
                    // A new one begins on its group: the old was cut short.
                    self.drop_sysex();
                }
                self.sysex_len = 0;
                self.sysex_over = false;
                self.sysex_group = Some(group);
                self.append(bytes);
                if status == COMPLETE {
                    self.finish(group)
                } else {
                    Decoded::Nothing
                }
            }
            CONTINUE | END if self.sysex_group == Some(group) => {
                self.append(bytes);
                if status == END {
                    self.finish(group)
                } else {
                    Decoded::Nothing
                }
            }
            // A part of a SysEx that never began here.
            _ => Decoded::Nothing,
        }
    }

    fn append(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            match self.sysex.get_mut(self.sysex_len) {
                Some(slot) => {
                    *slot = byte & 0x7F;
                    self.sysex_len = self.sysex_len.saturating_add(1);
                }
                None => self.sysex_over = true,
            }
        }
    }

    /// The SysEx under way is whole: it comes, unless it outgrew the buffer.
    fn finish(&mut self, group: Group) -> Decoded {
        self.sysex_group = None;
        if self.sysex_over {
            self.dropped = self.dropped.saturating_add(1);
            self.sysex_over = false;
            return Decoded::Nothing;
        }
        Decoded::SysEx(group)
    }

    fn drop_sysex(&mut self) {
        self.sysex_group = None;
        self.sysex_len = 0;
        self.sysex_over = false;
        self.dropped = self.dropped.saturating_add(1);
    }
}

/// The SysEx7 statuses: the whole message in one packet, or its first,
/// middle and last packets.
const COMPLETE: u32 = 0x0;
const START: u32 = 0x1;
const CONTINUE: u32 = 0x2;
const END: u32 = 0x3;

/// MIDI 2.0's registered per-note controller for a note's absolute pitch,
/// 7.25.
const PER_NOTE_PITCH: u8 = 3;
/// The registered parameter for the pitch bend range.
const BEND_RANGE: u8 = 0;

/// A 32-bit controller value as 0 to 1.
fn unit(value: u32) -> f32 {
    (f64::from(value) / f64::from(u32::MAX)) as f32
}

/// A pitch as MIDI 2.0's 7.25 carries it: 7 bits of key, 25 of fraction.
fn pitch_7_25(value: u32) -> Pitch {
    Pitch::new((f64::from(value) / f64::from(1_u32 << 25)) as f32)
}

/// The bend range registered parameter 0 carries: semitones in its 14
/// bits' high 7 and cents in their low 7, as MIDI 1.0 sends them.
fn semitones(value: u32) -> f32 {
    let entry = down_32_to_14(value);
    f32::from(entry >> 7) + f32::from(entry & 0x7F) / 100.0
}
