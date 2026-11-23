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
use hapto_ump::{
    Attribute, ChannelMessage, ChannelVoice, Event, MessageType, Origin, SysEx7, SysExForm,
};

use crate::message::{Message, NoteKey, NoteOff, NoteOn};
use crate::values::{Centred, Channel, Group, Key, NoteId, Pitch, Program};

/// How many notes a decoder made with [`Decoder::new`] keeps track of at
/// once, across every group and channel.
pub const NOTES: usize = 256;
/// The longest SysEx message a decoder made with [`Decoder::new`] joins.
pub const SYSEX_BYTES: usize = 4096;

/// Where a channel voice message came: its group and channel, and its
/// host's name for the note, if it gave one.
#[derive(Debug, Clone, Copy)]
struct Place {
    group: Group,
    channel: Channel,
    named: Option<NoteId>,
}

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
///   one, and a note-off ends every note on its key. A host gives one in
///   the packet's [`Origin::Note`], as VST3 names its notes: its note-off
///   ends that note alone, under the host's id whether or not the table
///   holds it, and a note's own message names that note, even when a newer
///   one sounds on its key. A note-off for a key with no note it knows of
///   still comes, under an id no note has, so a plugin that ends notes by key
///   hears it. All Notes Off, All Sound Off and the mode messages end a
///   channel's notes in the table. A note past the table takes the place of
///   the oldest, which is counted in [`dropped`](Self::dropped).
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
pub(crate) enum Decoded {
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

    /// The notes the last note-off ended.
    pub(crate) fn ended(&self) -> &[NoteId] {
        self.ends.get(..self.ended).unwrap_or(&[])
    }

    /// The SysEx last whole.
    pub(crate) fn sysex_bytes(&self) -> &[u8] {
        self.sysex.get(..self.sysex_len).unwrap_or(&[])
    }

    /// How many notes gave up their place in the full table, and how many
    /// SysEx messages were dropped, since it was made.
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

    /// What `event` decoded to, the note-offs' ids in [`ended`](Self::ended)
    /// and a whole SysEx in [`sysex_bytes`](Self::sysex_bytes).
    pub(crate) fn read(&mut self, event: &Event) -> Decoded {
        match event.message_type() {
            // Real time may fall inside a SysEx, and so may the groupless
            // utility and stream messages.
            MessageType::UTILITY | MessageType::STREAM => Decoded::Nothing,
            _ if event.is_real_time() => Decoded::Nothing,
            MessageType::SYSEX7 => match event.sysex7() {
                Some(packet) => self.sysex(packet),
                // A reserved status: part of no message.
                None => Decoded::Nothing,
            },
            _ => {
                // Anything else on a SysEx's group ends it, cut short.
                if self.sysex_group == Some(Group::field(event.group())) {
                    self.drop_sysex();
                }
                let named = match event.origin {
                    Origin::Note { id } => Some(NoteId::hosts(id)),
                    _ => None,
                };
                match ChannelVoice::decode(event.words) {
                    Some(voice) => self.voice(voice, named),
                    None => Decoded::Nothing,
                }
            }
        }
    }

    /// One channel voice message; `named` is its host's name for the note.
    fn voice(&mut self, voice: ChannelVoice, named: Option<NoteId>) -> Decoded {
        let group = Group::field(voice.group);
        let channel = Channel::field(voice.channel);
        let at = Place {
            group,
            channel,
            named,
        };
        if let Some(message) = self.per_note(at, voice.message) {
            return Decoded::One(message);
        }
        let message = match voice.message {
            ChannelMessage::NoteOn {
                note,
                velocity,
                attribute,
            } => Message::NoteOn(self.note_on(at, Key::new(note), velocity, attribute)),
            ChannelMessage::NoteOff { note, velocity, .. } => {
                return self.note_off(at, Key::new(note), velocity);
            }
            ChannelMessage::ControlChange { controller, value } => {
                // All Sound Off, All Notes Off and the mode messages end the
                // channel's notes, within its group.
                if matches!(controller, 120 | 123..=127) {
                    self.forget(group, channel);
                }
                Message::Control {
                    group,
                    channel,
                    controller,
                    value: unit(value),
                }
            }
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
    fn per_note(&mut self, at: Place, message: ChannelMessage) -> Option<Message<'static>> {
        let Place { group, channel, .. } = at;
        Some(match message {
            ChannelMessage::PolyPressure { note, value } => Message::NotePressure {
                note: self.key(at, note),
                value: unit(value),
            },
            ChannelMessage::RegisteredPerNoteController { note, index, value } => {
                let note = self.key(at, note);
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
                    note: self.key(at, note),
                    index,
                    value: unit(value),
                }
            }
            ChannelMessage::PerNotePitchBend { note, value } => Message::NoteBend {
                note: self.key(at, note),
                value: Centred::from_bits(value),
            },
            ChannelMessage::PerNoteManagement {
                note,
                detach,
                reset,
            } => {
                let key = self.key(at, note);
                if detach {
                    self.detach(group, channel, Key::new(note));
                }
                Message::NoteManagement {
                    note: key,
                    detach,
                    reset,
                }
            }
            _ => return None,
        })
    }

    fn note_on(&mut self, at: Place, key: Key, velocity: u16, attribute: Attribute) -> NoteOn {
        let Place {
            group,
            channel,
            named,
        } = at;
        let id = named.unwrap_or_else(|| self.make_id());
        if self.sounding >= self.notes.len() {
            // A full table: the oldest note gives up its place, so the
            // newest, which a note-off is likeliest to name, is kept.
            self.dropped = self.dropped.saturating_add(1);
            if !self.notes.is_empty() {
                self.notes.copy_within(1.., 0);
                self.sounding = self.sounding.saturating_sub(1);
            }
        }
        if let Some(slot) = self.notes.get_mut(self.sounding) {
            *slot = Sounding {
                group,
                channel,
                key,
                id,
                detached: false,
            };
            self.sounding = self.sounding.saturating_add(1);
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

    /// Ends every note on the key, or the note its host named alone, moving
    /// their ids to `ends`. A named note the table does not hold, one it
    /// lost or one from before a reset, ends under the host's id, and no
    /// other note with it.
    fn note_off(&mut self, at: Place, key: Key, velocity: u16) -> Decoded {
        let Place {
            group,
            channel,
            named,
        } = at;
        let on_key =
            |note: &Sounding| (note.group, note.channel, note.key) == (group, channel, key);
        let only = named;
        self.ended = 0;
        let mut kept = 0;
        for index in 0..self.sounding {
            let Some(&note) = self.notes.get(index) else {
                break;
            };
            if on_key(&note) && only.is_none_or(|id| note.id == id) {
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
            // No note it knows of: the host's name for it, or an id no note
            // has.
            let id = named.unwrap_or_else(|| self.make_id());
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

    /// The key a per-note message names, with its note: the one its host
    /// named, if that one sounds there, and none if not; with no name, the
    /// newest on the key. A detached note takes no per-note messages, so it
    /// is never named.
    fn key(&self, at: Place, key: u8) -> NoteKey {
        let Place {
            group,
            channel,
            named,
        } = at;
        let key = Key::new(key);
        let mut on_key = self
            .notes
            .get(..self.sounding)
            .unwrap_or(&[])
            .iter()
            .rev()
            .filter(|note| (note.group, note.channel, note.key) == (group, channel, key));
        let note = match named {
            Some(id) => on_key.find(|note| note.id == id),
            None => on_key.next(),
        };
        let id = note.filter(|note| !note.detached).map(|note| note.id);
        NoteKey {
            group,
            channel,
            key,
            id,
        }
    }

    /// Forgets every note on `group` and `channel`.
    fn forget(&mut self, group: Group, channel: Channel) {
        let mut kept = 0;
        for index in 0..self.sounding {
            let Some(&note) = self.notes.get(index) else {
                break;
            };
            if (note.group, note.channel) != (group, channel) {
                if let Some(slot) = self.notes.get_mut(kept) {
                    *slot = note;
                }
                kept = kept.saturating_add(1);
            }
        }
        self.sounding = kept;
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
        let id = NoteId::made_up(self.made);
        self.made = self.made.wrapping_add(1);
        id
    }

    /// One 7-bit SysEx packet.
    fn sysex(&mut self, packet: SysEx7) -> Decoded {
        let group = Group::field(packet.group);
        let bytes = packet.data();
        match packet.form {
            SysExForm::Complete | SysExForm::Start => {
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
                if packet.form.ends() {
                    self.finish(group)
                } else {
                    Decoded::Nothing
                }
            }
            SysExForm::Continue | SysExForm::End if self.sysex_group == Some(group) => {
                self.append(bytes);
                if packet.form.ends() {
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
