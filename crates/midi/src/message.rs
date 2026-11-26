//! Decoded notes and controllers in MIDI 2.0's meanings, each value in the
//! type that it stands for.

use std::fmt;

use hapto_ump::Attribute;

use crate::controller::{self, Address};
use crate::values::{Centred, Channel, Group, Key, NoteId, Pitch, Program};

/// A note starting.
///
/// # Examples
///
/// ```
/// use hapto_midi::{Key, NoteOn};
///
/// // A note to play a voice on its own in a test.
/// let note = NoteOn::new(Key::A4, 0.8);
/// assert_eq!(note.pitch().hz(), 440.0);
/// let off = note.off(0.5);
/// assert_eq!((off.id, off.key), (note.id, Key::A4));
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteOn {
    /// The note's id, until it ends.
    pub id: NoteId,
    /// Its group.
    pub group: Group,
    /// Its channel.
    pub channel: Channel,
    /// The key.
    pub key: Key,
    /// From 0 to 1. A MIDI 2.0 note-on at velocity 0 is still a note-on, but
    /// the upgrade turns a MIDI 1.0 one into a note-off.
    pub velocity: f32,
    pub(crate) attribute: Attribute,
}

impl NoteOn {
    /// Returns a note-on for `key` on the first group and channel, as the
    /// plugin's own note 0 with no attribute, for testing a voice on its own.
    pub fn new(key: Key, velocity: f32) -> Self {
        NoteOn {
            id: NoteId::own(0),
            group: Group::FIRST,
            channel: Channel::FIRST,
            key,
            velocity,
            attribute: Attribute::NONE,
        }
    }

    /// Returns this note moved to `key`, as a transposer sends it.
    ///
    /// A pitch attribute moves by the same interval, clamped to MIDI's range,
    /// so the note sounds at `key`. The note keeps its id, so its host takes
    /// it for the original, and a plugin sends it in that note's place. A
    /// note sent beside the original takes an id of the plugin's own, with
    /// [`with_id`](Self::with_id).
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_midi::{Key, NoteOn};
    ///
    /// let note = NoteOn::new(Key::new(60), 1.0).with_key(Key::new(67));
    /// assert_eq!(note.key, Key::new(67));
    /// assert_eq!(note.pitch().semitones(), 67.0);
    /// ```
    #[must_use]
    pub fn with_key(self, key: Key) -> Self {
        let mut attribute = self.attribute;
        if let Some(semitones) = attribute.semitones() {
            let keys = f64::from(key.wire()) - f64::from(self.key.wire());
            attribute = Attribute::pitch(semitones + keys);
        }
        NoteOn {
            key,
            attribute,
            ..self
        }
    }

    /// Returns this note under `id`, for a note sent beside the original
    /// under an id of the plugin's own ([`NoteId::own`]).
    #[must_use]
    pub fn with_id(self, id: NoteId) -> Self {
        NoteOn { id, ..self }
    }

    /// Returns the note-off that ends this note, at `velocity`.
    pub fn off(&self, velocity: f32) -> NoteOff {
        NoteOff {
            id: self.id,
            group: self.group,
            channel: self.channel,
            key: self.key,
            velocity,
        }
    }

    /// Returns the note's pitch, which is its key in equal temperament
    /// unless the note-on carries MIDI 2.0's pitch attribute.
    pub fn pitch(&self) -> Pitch {
        match self.attribute.semitones() {
            Some(semitones) => Pitch::new(semitones as f32),
            None => Pitch::from(self.key),
        }
    }

    /// Returns the same note at `pitch`, through MIDI 2.0's pitch attribute,
    /// whatever its key.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_midi::{Key, NoteOn, Pitch};
    ///
    /// // A4 a quarter tone sharp.
    /// let note = NoteOn::new(Key::new(69), 1.0).with_pitch(Pitch::new(69.5));
    /// assert_eq!(note.pitch().semitones(), 69.5);
    /// ```
    #[must_use]
    pub fn with_pitch(self, pitch: Pitch) -> Self {
        NoteOn {
            attribute: Attribute::pitch(f64::from(pitch.semitones())),
            ..self
        }
    }

    /// Returns the note's attribute as it came, for one that Hapto doesn't
    /// read, or `None` if there is none.
    pub fn attribute(&self) -> Option<Attribute> {
        (self.attribute != Attribute::NONE).then_some(self.attribute)
    }
}

/// A note ending.
///
/// # Examples
///
/// ```
/// use hapto_midi::{Key, NoteOn};
///
/// let middle_c = Key::C4;
/// let off = NoteOn::new(middle_c, 1.0).off(0.25);
/// assert_eq!(off.velocity, 0.25);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteOff {
    /// The note that ends.
    pub id: NoteId,
    /// Its group.
    pub group: Group,
    /// Its channel.
    pub channel: Channel,
    /// Its key.
    pub key: Key,
    /// From 0 to 1, or 0.5 if the sender gave none, as a MIDI 1.0 note-on at
    /// velocity 0 upgrades.
    pub velocity: f32,
}

/// The key that a per-note message names, and the note sounding on it.
///
/// Match a voice by its [`id`](Self::id), which the decoder has found, not
/// by its key, because two notes can sound on one key.
///
/// MIDI 2.0 addresses a per-note message by key, and the message can come
/// before the key's note, so it can name a key with no note. The note that
/// it names:
///
/// - With two notes on one key, it is the newer, because under MIDI 2.0's
///   per-note management the older takes no more per-note messages.
/// - A host that names its notes, as VST3 does, names the note itself, older
///   or not.
/// - A note that no longer sounds, or that is detached, gives none.
///
/// # Examples
///
/// ```
/// use hapto_midi::{Decoder, Message};
/// use hapto_ump::Event;
///
/// // Pressure on C4 with no note sounding there.
/// let pressure = Event::new(0, [0x40A0_3C00, 0x8000_0000, 0, 0]);
/// let mut decoder = Decoder::new();
/// let Some(Message::NotePressure { note, .. }) = decoder.decode(&pressure).next() else {
///     panic!("a note's pressure");
/// };
/// assert_eq!((note.key.to_string(), note.id), ("C4".to_owned(), None));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NoteKey {
    /// Its group.
    pub group: Group,
    /// Its channel.
    pub channel: Channel,
    /// The key.
    pub key: Key,
    /// The note that it is for, or `None` if there is none.
    pub id: Option<NoteId>,
}

/// One decoded note or controller change, in MIDI 2.0's meanings.
///
/// Its `Display` writes one line in musicians' numbering. A value from 0 to
/// 1 is an `f32`, and one that rests at a centre is [`Centred`]. A per-note
/// message names its [`NoteKey`].
///
/// # Examples
///
/// ```
/// use hapto_midi::controller::number;
/// use hapto_midi::{Decoder, Message};
/// use hapto_ump::Event;
///
/// // MIDI 2.0's controller 64, the sustain pedal, all the way down.
/// let pedal = Event::new(0, [0x40B0_4000, u32::MAX, 0, 0]);
/// let mut decoder = Decoder::new();
/// let message = decoder.decode(&pedal).next().expect("a controller");
/// assert!(matches!(
///     message,
///     Message::Control { controller: number::SUSTAIN, value: 1.0, .. }
/// ));
/// assert_eq!(message.to_string(), "controller 64 (Sustain Pedal) 1.000, channel 1, group 1");
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum Message<'a> {
    /// A note starting.
    NoteOn(NoteOn),
    /// A note ending, one for each note that a note-off ends.
    NoteOff(NoteOff),
    /// A note's absolute pitch, to which no bend range applies (MIDI 2.0's
    /// registered per-note controller 3).
    NotePitch {
        /// The key, and its note.
        note: NoteKey,
        /// The pitch.
        pitch: Pitch,
    },
    /// A note's own pitch bend, at rest at the centre.
    NoteBend {
        /// The key, and its note.
        note: NoteKey,
        /// The bend.
        value: Centred,
    },
    /// A note's own pressure (polyphonic aftertouch).
    NotePressure {
        /// The key, and its note.
        note: NoteKey,
        /// From 0 to 1.
        value: f32,
    },
    /// A registered per-note controller, numbered as a channel's are, so 74
    /// is brightness and 10 pan.
    NoteControl {
        /// The key, and its note.
        note: NoteKey,
        /// The controller, 0 to 255, which [`controller`] names.
        controller: u8,
        /// From 0 to 1.
        value: f32,
    },
    /// An assignable per-note controller, which the instrument defines.
    NoteAssignable {
        /// The key, and its note.
        note: NoteKey,
        /// The controller, 0 to 255.
        index: u8,
        /// From 0 to 1.
        value: f32,
    },
    /// MIDI 2.0's per-note management for a key.
    NoteManagement {
        /// The key, and its note.
        note: NoteKey,
        /// The key's sounding notes take no more per-note messages.
        detach: bool,
        /// The key's per-note controllers return to their defaults.
        reset: bool,
    },
    /// The channel's pitch bend, at rest at the centre.
    Bend {
        /// Its group.
        group: Group,
        /// Its channel.
        channel: Channel,
        /// The bend.
        value: Centred,
    },
    /// How far a full bend reaches, in semitones either way (registered
    /// parameter 0).
    BendRange {
        /// Its group.
        group: Group,
        /// Its channel.
        channel: Channel,
        /// The semitones, cents as a fraction.
        semitones: f32,
    },
    /// The channel's pressure (channel aftertouch).
    Pressure {
        /// Its group.
        group: Group,
        /// Its channel.
        channel: Channel,
        /// From 0 to 1.
        value: f32,
    },
    /// A control change.
    Control {
        /// Its group.
        group: Group,
        /// Its channel.
        channel: Channel,
        /// The controller, 0 to 127, which [`controller`] names.
        controller: u8,
        /// From 0 to 1.
        value: f32,
    },
    /// A registered controller (MIDI 1.0's RPN), except the bend range,
    /// which has its own message.
    Registered {
        /// Its group.
        group: Group,
        /// Its channel.
        channel: Channel,
        /// The bank, 0 to 127.
        bank: u8,
        /// The controller in its bank, 0 to 127.
        index: u8,
        /// From 0 to 1.
        value: f32,
    },
    /// An assignable controller (MIDI 1.0's NRPN), which the instrument
    /// defines.
    Assignable {
        /// Its group.
        group: Group,
        /// Its channel.
        channel: Channel,
        /// The bank, 0 to 127.
        bank: u8,
        /// The controller in its bank, 0 to 127.
        index: u8,
        /// From 0 to 1.
        value: f32,
    },
    /// A program change, with the bank that it selects, if any.
    Program {
        /// Its group.
        group: Group,
        /// Its channel.
        channel: Channel,
        /// The program.
        program: Program,
        /// The bank, its two 7-bit halves as one number, 0 to 16383.
        bank: Option<u16>,
    },
    /// A whole SysEx message, without its F0 and F7.
    SysEx {
        /// Its group.
        group: Group,
        /// Its bytes, from the decoder's buffer.
        data: &'a [u8],
    },
}

impl From<NoteOn> for Message<'_> {
    fn from(note: NoteOn) -> Self {
        Message::NoteOn(note)
    }
}

impl From<NoteOff> for Message<'_> {
    fn from(off: NoteOff) -> Self {
        Message::NoteOff(off)
    }
}

/// A message's group and channel, written as ", channel 1, group 1".
struct On(Group, Channel);

impl<'a> Message<'a> {
    /// Returns the message's group.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_midi::{Decoder, Group};
    /// use hapto_ump::Event;
    ///
    /// // A controller on group 3.
    /// let event = Event::new(0, [0x42B0_0700, u32::MAX, 0, 0]);
    /// let mut decoder = Decoder::new();
    /// let message = decoder.decode(&event).next().expect("a controller");
    /// assert_eq!(message.group(), Group::number(3));
    /// ```
    pub fn group(&self) -> Group {
        match *self {
            Message::NoteOn(NoteOn { group, .. }) | Message::NoteOff(NoteOff { group, .. }) => {
                group
            }
            Message::NotePitch { note, .. }
            | Message::NoteBend { note, .. }
            | Message::NotePressure { note, .. }
            | Message::NoteControl { note, .. }
            | Message::NoteAssignable { note, .. }
            | Message::NoteManagement { note, .. } => note.group,
            Message::Bend { group, .. }
            | Message::BendRange { group, .. }
            | Message::Pressure { group, .. }
            | Message::Control { group, .. }
            | Message::Registered { group, .. }
            | Message::Assignable { group, .. }
            | Message::Program { group, .. }
            | Message::SysEx { group, .. } => group,
        }
    }

    /// Returns the message's channel, or `None` for a SysEx.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_midi::{Channel, Decoder, Message};
    /// use hapto_ump::Event;
    ///
    /// // Listening only on channel 2.
    /// let event = Event::new(0, [0x40B1_0700, u32::MAX, 0, 0]);
    /// let mut decoder = Decoder::new();
    /// let mine = |message: &Message<'_>| message.channel() == Some(Channel::number(2));
    /// assert!(decoder.decode(&event).all(|message| mine(&message)));
    /// ```
    pub fn channel(&self) -> Option<Channel> {
        Some(match *self {
            Message::NoteOn(NoteOn { channel, .. }) | Message::NoteOff(NoteOff { channel, .. }) => {
                channel
            }
            Message::Bend { channel, .. }
            | Message::BendRange { channel, .. }
            | Message::Pressure { channel, .. }
            | Message::Control { channel, .. }
            | Message::Registered { channel, .. }
            | Message::Assignable { channel, .. }
            | Message::Program { channel, .. } => channel,
            Message::SysEx { .. } => return None,
            _ => self.note()?.channel,
        })
    }

    /// Returns the key and note that a note's message names, including a
    /// note-on or note-off, or `None` for a channel's message or a SysEx.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_midi::{Decoder, Key};
    /// use hapto_ump::Event;
    ///
    /// // A note-on for C4, then its pressure.
    /// let mut decoder = Decoder::new();
    /// let on = Event::new(0, [0x4090_3C00, 0xFFFF_0000, 0, 0]);
    /// let pressure = Event::new(0, [0x40A0_3C00, 0x8000_0000, 0, 0]);
    /// let id = decoder.decode(&on).next().and_then(|message| message.note()).and_then(|note| note.id);
    /// let note = decoder.decode(&pressure).next().and_then(|message| message.note()).expect("a note's");
    /// assert_eq!((note.key, note.id), (Key::new(60), id));
    /// ```
    pub fn note(&self) -> Option<NoteKey> {
        let event = |group, channel, key, id| NoteKey {
            group,
            channel,
            key,
            id: Some(id),
        };
        match *self {
            Message::NoteOn(on) => Some(event(on.group, on.channel, on.key, on.id)),
            Message::NoteOff(off) => Some(event(off.group, off.channel, off.key, off.id)),
            Message::NotePitch { note, .. }
            | Message::NoteBend { note, .. }
            | Message::NotePressure { note, .. }
            | Message::NoteControl { note, .. }
            | Message::NoteAssignable { note, .. }
            | Message::NoteManagement { note, .. } => Some(note),
            _ => None,
        }
    }

    /// Returns the controller that a control change, registered or assignable
    /// controller sets, and its value from 0 to 1, or `None` for any other
    /// message, including a per-note controller.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_midi::controller::{self, number};
    /// use hapto_midi::Decoder;
    /// use hapto_ump::Event;
    ///
    /// let event = Event::new(0, [0x40B0_0700, u32::MAX, 0, 0]);
    /// let mut decoder = Decoder::new();
    /// let message = decoder.decode(&event).next().expect("a controller");
    /// let address = controller::Address::Control(number::VOLUME);
    /// assert_eq!(message.controller(), Some((address, 1.0)));
    /// ```
    pub fn controller(&self) -> Option<(Address, f32)> {
        match *self {
            Message::Control {
                controller, value, ..
            } => Some((Address::Control(controller), value)),
            Message::Registered {
                bank, index, value, ..
            } => Some((Address::Registered { bank, index }, value)),
            Message::Assignable {
                bank, index, value, ..
            } => Some((Address::Assignable { bank, index }, value)),
            _ => None,
        }
    }
}

impl fmt::Display for On {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, ", channel {}, group {}", self.1, self.0)
    }
}

impl fmt::Display for NoteKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.key, On(self.group, self.channel))?;
        match self.id {
            Some(id) => write!(f, ", {id}"),
            None => f.write_str(", no note"),
        }
    }
}

/// A controller's number, with its name where MIDI 1.0 gives one.
struct Named(u8);

impl fmt::Display for Named {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match controller::name(self.0) {
            Some(name) => write!(f, "{} ({name})", self.0),
            None => write!(f, "{}", self.0),
        }
    }
}

/// The most bytes of a SysEx message that its line shows.
const SHOWN_BYTES: usize = 16;

/// Writes a note-on or note-off as its line.
fn fmt_event(
    f: &mut fmt::Formatter<'_>,
    what: &str,
    key: Key,
    velocity: f32,
    on: On,
    id: NoteId,
) -> fmt::Result {
    write!(f, "{what} {key}, velocity {velocity:.3}{on}, {id}")
}

/// One line in musicians' numbers, the same as a [`Message::NoteOn`]'s.
impl fmt::Display for NoteOn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let on = On(self.group, self.channel);
        fmt_event(f, "note on", self.key, self.velocity, on, self.id)
    }
}

/// One line in musicians' numbers, the same as a [`Message::NoteOff`]'s.
impl fmt::Display for NoteOff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let on = On(self.group, self.channel);
        fmt_event(f, "note off", self.key, self.velocity, on, self.id)
    }
}

impl fmt::Display for Message<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Message::NoteOn(on) => on.fmt(f),
            Message::NoteOff(off) => off.fmt(f),
            Message::NotePitch { .. }
            | Message::NoteBend { .. }
            | Message::NotePressure { .. }
            | Message::NoteControl { .. }
            | Message::NoteAssignable { .. }
            | Message::NoteManagement { .. } => self.fmt_note(f),
            Message::Bend {
                group,
                channel,
                value,
            } => write!(f, "pitch bend {value}{}", On(group, channel)),
            Message::BendRange {
                group,
                channel,
                semitones,
            } => write!(
                f,
                "pitch bend range {semitones:.2} semitones{}",
                On(group, channel)
            ),
            Message::Pressure {
                group,
                channel,
                value,
            } => write!(f, "channel pressure {value:.3}{}", On(group, channel)),
            Message::Control {
                group,
                channel,
                controller,
                value,
            } => write!(
                f,
                "controller {} {value:.3}{}",
                Named(controller),
                On(group, channel)
            ),
            Message::Registered {
                group,
                channel,
                bank,
                index,
                value,
            } => write!(
                f,
                "registered controller {bank}:{index} {value:.3}{}",
                On(group, channel)
            ),
            Message::Assignable {
                group,
                channel,
                bank,
                index,
                value,
            } => write!(
                f,
                "assignable controller {bank}:{index} {value:.3}{}",
                On(group, channel)
            ),
            Message::Program {
                group,
                channel,
                program,
                bank,
            } => {
                write!(f, "program {program}")?;
                if let Some(bank) = bank {
                    write!(f, ", bank {bank}")?;
                }
                write!(f, "{}", On(group, channel))
            }
            Message::SysEx { group, data } => {
                write!(f, "SysEx of {} bytes:", data.len())?;
                for byte in data.iter().take(SHOWN_BYTES) {
                    write!(f, " {byte:02X}")?;
                }
                if data.len() > SHOWN_BYTES {
                    f.write_str(" ...")?;
                }
                write!(f, ", group {group}")
            }
        }
    }
}

impl Message<'_> {
    /// Writes a per-note message as its line.
    fn fmt_note(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Message::NotePitch { note, pitch } => write!(f, "pitch {pitch} for {note}"),
            Message::NoteBend { note, value } => write!(f, "pitch bend {value} for {note}"),
            Message::NotePressure { note, value } => write!(f, "pressure {value:.3} for {note}"),
            Message::NoteControl {
                note,
                controller,
                value,
            } => write!(f, "controller {} {value:.3} for {note}", Named(controller)),
            Message::NoteAssignable { note, index, value } => {
                write!(f, "assignable controller {index} {value:.3} for {note}")
            }
            Message::NoteManagement {
                note,
                detach,
                reset,
            } => write!(
                f,
                "note management, detach {detach}, reset {reset}, for {note}"
            ),
            // Display writes these itself.
            Message::NoteOn(_)
            | Message::NoteOff(_)
            | Message::Bend { .. }
            | Message::BendRange { .. }
            | Message::Pressure { .. }
            | Message::Control { .. }
            | Message::Registered { .. }
            | Message::Assignable { .. }
            | Message::Program { .. }
            | Message::SysEx { .. } => Ok(()),
        }
    }
}
