//! The values a message carries, each its own type: stored as MIDI sends
//! them, and shown as musicians count them.

use std::fmt;

/// A key, 0 to 127, shown by name: middle C, 60, is C4.
///
/// # Examples
///
/// ```
/// use hapto_midi::Key;
///
/// assert_eq!(Key::C4.number(), 60);
/// assert_eq!(Key::new(61).to_string(), "C#4");
/// assert_eq!(Key::new(0).to_string(), "C-1");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key(u8);

impl Key {
    /// Middle C.
    pub const C4: Key = Key(60);
    /// A4: 440 Hz in equal temperament.
    pub const A4: Key = Key(69);

    /// The key numbered `number` as MIDI sends it, masked to 7 bits.
    pub const fn new(number: u8) -> Self {
        Key(number & 0x7F)
    }

    /// Its number, as MIDI sends it.
    pub const fn number(self) -> u8 {
        self.0
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const NAMES: [&str; 12] = [
            "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
        ];
        let name = NAMES.get(usize::from(self.0 % 12)).copied().unwrap_or("?");
        write!(f, "{name}{}", i16::from(self.0 / 12).saturating_sub(1))
    }
}

/// A UMP group: 0 to 15 as a packet carries it, shown 1 to 16.
///
/// # Examples
///
/// ```
/// use hapto_midi::Group;
///
/// assert_eq!(Group::FIRST, Group::number(1));
/// assert_eq!(Group::number(16).to_string(), "16");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Group(u8);

impl Group {
    /// The first group, shown as 1: where MIDI from a source with no groups
    /// arrives.
    pub const FIRST: Group = Group(0);

    /// The group a musician counts as `number`, 1 to 16.
    ///
    /// # Panics
    ///
    /// If `number` is not 1 to 16: in a `const`, at compile time.
    pub const fn number(number: u8) -> Self {
        assert!(number >= 1 && number <= 16, "groups are 1 to 16");
        Group(number.saturating_sub(1))
    }

    /// The group a packet's group field names, 0 to 15.
    pub(crate) const fn field(field: u8) -> Self {
        Group(field & 0xF)
    }

    /// Its place among the 16, 0 to 15, for indexing a table of them.
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Display for Group {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", u16::from(self.0) + 1)
    }
}

/// A channel: 0 to 15 as a packet carries it, shown 1 to 16 as musicians
/// count.
///
/// # Examples
///
/// ```
/// use hapto_midi::Channel;
///
/// // MIDI's drum channel is the tenth.
/// let drums = Channel::number(10);
/// assert_eq!((drums.index(), drums.to_string()), (9, "10".to_owned()));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Channel(u8);

impl Channel {
    /// The first channel, shown as 1.
    pub const FIRST: Channel = Channel(0);

    /// The channel a musician counts as `number`, 1 to 16.
    ///
    /// # Panics
    ///
    /// If `number` is not 1 to 16: in a `const`, at compile time.
    pub const fn number(number: u8) -> Self {
        assert!(number >= 1 && number <= 16, "channels are 1 to 16");
        Channel(number.saturating_sub(1))
    }

    /// The channel a packet's channel field names, 0 to 15.
    pub(crate) const fn field(field: u8) -> Self {
        Channel(field & 0xF)
    }

    /// Its place among a group's 16 channels, 0 to 15, for indexing a table
    /// of them.
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", u16::from(self.0) + 1)
    }
}

/// Which of its stored sounds a program change asks an instrument for: 0 to
/// 127 as MIDI sends it, and shown 1 to 128 as General MIDI numbers them, so
/// program 1 is the Acoustic Grand Piano.
///
/// # Examples
///
/// ```
/// use hapto_midi::Program;
///
/// let piano = Program::number(1);
/// assert_eq!((piano.wire(), piano.to_string()), (0, "1".to_owned()));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Program(u8);

impl Program {
    /// The program a keyboard shows as `number`, 1 to 128.
    ///
    /// # Panics
    ///
    /// If `number` is not 1 to 128: in a `const`, at compile time.
    pub const fn number(number: u8) -> Self {
        assert!(number >= 1 && number <= 128, "programs are 1 to 128");
        Program(number.saturating_sub(1))
    }

    /// The program MIDI sends as `wire`, 0 to 127, masked to 7 bits.
    pub const fn from_wire(wire: u8) -> Self {
        Program(wire & 0x7F)
    }

    /// The number as MIDI sends it, 0 to 127.
    pub const fn wire(self) -> u8 {
        self.0
    }
}

impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", u16::from(self.0) + 1)
    }
}

/// A value with a resting centre, from -1 to 1 and 0 at rest: a pitch bend,
/// a brightness, a pan.
///
/// MIDI 2.0 sends one as 32 bits with its centre at `0x8000_0000`, which is
/// exactly 0 here, as the lowest and highest values are exactly -1 and 1.
///
/// # Examples
///
/// ```
/// use hapto_midi::Centred;
///
/// assert_eq!(Centred::new(0.25).get(), 0.25);
/// // Past the ends it stops at them.
/// assert_eq!(Centred::new(3.0).get(), 1.0);
/// assert_eq!(Centred::default(), Centred::CENTRE);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Centred(f32);

impl Centred {
    /// At rest.
    pub const CENTRE: Centred = Centred(0.0);

    /// `value`, held to -1 to 1; a value that is not a number is the
    /// centre.
    pub fn new(value: f32) -> Self {
        Centred(held(value, -1.0, 1.0, 0.0))
    }

    /// The value MIDI 2.0's 32 bits carry, its centre at `0x8000_0000`.
    pub(crate) fn from_bits(bits: u32) -> Self {
        const CENTRE: u32 = 0x8000_0000;
        let value = if bits >= CENTRE {
            f64::from(bits.saturating_sub(CENTRE)) / f64::from(u32::MAX - CENTRE)
        } else {
            -(f64::from(CENTRE.saturating_sub(bits)) / f64::from(CENTRE))
        };
        Centred(value as f32)
    }

    /// The value, from -1 to 1.
    pub fn get(self) -> f32 {
        self.0
    }
}

impl fmt::Display for Centred {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:+.3}", self.0)
    }
}

/// A pedal's place, from 0, up, to 1, down: the damper, sostenuto and soft
/// pedals. MIDI 1.0 reads a pedal as a switch, down from the middle on,
/// which [`is_down`](Self::is_down) says, and a keyboard that sends half
/// pedal sends the places between, which [`depth`](Self::depth) gives.
///
/// # Examples
///
/// ```
/// use hapto_midi::Pedal;
///
/// // Half pedal: down, as MIDI 1.0 reads a switch, and half way.
/// let half = Pedal::new(0.5);
/// assert!(half.is_down());
/// assert_eq!(half.depth(), 0.5);
/// assert!(!Pedal::UP.is_down());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Pedal(f32);

impl Pedal {
    /// All the way up: the damper on the strings.
    pub const UP: Pedal = Pedal(0.0);
    /// All the way down.
    pub const DOWN: Pedal = Pedal(1.0);

    /// A pedal `depth` down, held to 0 to 1; a depth that is not a number
    /// is up.
    pub fn new(depth: f32) -> Self {
        Pedal(held(depth, 0.0, 1.0, 0.0))
    }

    /// Whether it is down, as MIDI 1.0 reads a pedal: from the middle on.
    pub fn is_down(self) -> bool {
        self.0 >= 0.5
    }

    /// How far down it is, from 0 to 1.
    pub fn depth(self) -> f32 {
        self.0
    }
}

impl fmt::Display for Pedal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let place = if self.is_down() { "down" } else { "up" };
        write!(f, "{place} ({:.3})", self.0)
    }
}

/// A pitch as a key with a fraction: A4 is 69.0, a quarter tone above it
/// 69.5.
///
/// # Examples
///
/// ```
/// use hapto_midi::{Key, Pitch};
///
/// assert_eq!(Pitch::of(Key::A4).hz(), 440.0);
/// assert!((Pitch::new(81.0).hz() - 880.0).abs() < 1e-3);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Pitch(f32);

impl Pitch {
    /// The pitch `semitones` above key 0.
    pub const fn new(semitones: f32) -> Self {
        Pitch(semitones)
    }

    /// The pitch of `key` in equal temperament.
    pub fn of(key: Key) -> Self {
        Pitch(f32::from(key.0))
    }

    /// The pitch in semitones above key 0.
    pub const fn semitones(self) -> f32 {
        self.0
    }

    /// The frequency in Hz, with A4 at 440 Hz.
    pub fn hz(self) -> f32 {
        440.0 * ((self.0 - 69.0) / 12.0).exp2()
    }
}

impl fmt::Display for Pitch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3} semitones", self.0)
    }
}

/// A sounding note, as Hapto names it from its note-on to its note-off.
///
/// It is the host's own id where the format has one, VST3's `noteId` say.
/// Elsewhere the decoder makes one up, with the top bit set, so it never
/// meets a host's.
///
/// # Examples
///
/// ```
/// use hapto_midi::{Decoder, Message};
/// use hapto_ump::Event;
///
/// // A note-on with no host id gets one made up.
/// let mut decoder = Decoder::new();
/// let on = Event::new(0, [0x4090_3C00, 0xFFFF_0000, 0, 0]);
/// let Some(Message::NoteOn(note)) = decoder.decode(&on).next() else {
///     panic!("a note-on");
/// };
/// assert!(!note.id.is_hosts());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NoteId(u32);

/// The bit every made-up id has, and no host's.
const MADE_UP: u32 = 0x8000_0000;
/// The bits a plugin's own id has: the made-up bit and the one below it,
/// which the decoder's ids never set.
const OWN: u32 = 0xC000_0000;

impl NoteId {
    /// The host's id `id`, below `0x8000_0000`.
    pub const fn hosts(id: u32) -> Self {
        NoteId(id & !MADE_UP)
    }

    /// An id of the plugin's own, `n` of up to 30 bits, for a note it sends
    /// beside the ones it reads: a chord's other notes, say. It is never a
    /// host's, so its note goes out under no host id, nor one the decoder
    /// makes up, so a plugin may keep both in one table.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_midi::{Key, NoteId, NoteOn};
    ///
    /// // A fifth above a note the host named, as a note of its own.
    /// let root = NoteOn::new(Key::new(60), 1.0).with_id(NoteId::hosts(7));
    /// let fifth = root.with_key(Key::new(67)).with_id(NoteId::own(1));
    /// assert!(!fifth.id.is_hosts());
    /// assert_ne!(fifth.id, root.id);
    /// ```
    pub const fn own(n: u32) -> Self {
        NoteId(n & !OWN | OWN)
    }

    /// The `count`th id made up, wrapping below a plugin's own.
    pub(crate) const fn made_up(count: u32) -> Self {
        NoteId(count & !OWN | MADE_UP)
    }

    /// Whether the host named the note, rather than Hapto.
    pub const fn is_hosts(self) -> bool {
        self.0 & MADE_UP == 0
    }

    /// The id as one number: a host's below `0x8000_0000`, Hapto's at or
    /// above it.
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for NoteId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_hosts() {
            write!(f, "host's note {}", self.0)
        } else if self.0 & OWN == OWN {
            write!(f, "plugin's note {}", self.0 & !OWN)
        } else {
            write!(f, "note {}", self.0 & !MADE_UP)
        }
    }
}

/// `value` held to `low` to `high`, or `if_nan` for a value that is not a
/// number.
pub(crate) fn held(value: f32, low: f32, high: f32, if_nan: f32) -> f32 {
    if value.is_nan() {
        if_nan
    } else {
        value.clamp(low, high)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_shows_its_name_and_octave() {
        let names: Vec<String> = [0, 11, 12, 59, 60, 69, 127]
            .map(|n| Key::new(n).to_string())
            .into();
        assert_eq!(names, ["C-1", "B-1", "C0", "B3", "C4", "A4", "G9"]);
    }

    #[test]
    fn a_centred_value_lands_on_its_ends_and_centre_exactly() {
        assert_eq!(Centred::from_bits(0).get(), -1.0);
        assert_eq!(Centred::from_bits(0x8000_0000).get(), 0.0);
        assert_eq!(Centred::from_bits(u32::MAX).get(), 1.0);
        assert!(Centred::from_bits(0xC000_0000).get() > 0.49);
        assert_eq!(Centred::new(f32::NAN), Centred::CENTRE);
    }

    #[test]
    fn a_hosts_id_and_a_made_up_one_never_meet() {
        assert!(NoteId::hosts(7).is_hosts());
        assert!(!NoteId::made_up(7).is_hosts());
        assert_ne!(NoteId::hosts(7), NoteId::made_up(7));
        assert_eq!(NoteId::hosts(u32::MAX).get(), 0x7FFF_FFFF);
        assert_eq!(NoteId::made_up(3).to_string(), "note 3");
        assert_eq!(NoteId::hosts(3).to_string(), "host's note 3");
    }
}
