//! The values that a message carries, each its own type, stored as MIDI
//! sends them and shown as musicians count them.

use std::fmt;

/// A key, 0 to 127, shown by name, so middle C (60) is C4.
///
/// # Examples
///
/// ```
/// use hapto_midi::Key;
///
/// assert_eq!(Key::C4.wire(), 60);
/// assert_eq!(Key::new(61).to_string(), "C#4");
/// assert_eq!(Key::new(0).to_string(), "C-1");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key(u8);

impl Key {
    /// Middle C.
    pub const C4: Key = Key(60);
    /// A4, at 440 Hz in equal temperament.
    pub const A4: Key = Key(69);

    /// Returns the key that MIDI numbers `number`, masked to 7 bits.
    pub const fn new(number: u8) -> Self {
        Key(number & 0x7F)
    }

    /// Returns the number that MIDI sends, 0 to 127.
    pub const fn wire(self) -> u8 {
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

/// A UMP group, carried as 0 to 15 and shown as 1 to 16.
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
    /// The first group, shown as 1, where MIDI from a source without groups
    /// arrives.
    pub const FIRST: Group = Group(0);

    /// Returns the group that a musician counts as `number`, 1 to 16.
    ///
    /// # Panics
    ///
    /// If `number` is not 1 to 16: in a `const`, at compile time.
    pub const fn number(number: u8) -> Self {
        assert!(number >= 1 && number <= 16, "groups are 1 to 16");
        Group(number.saturating_sub(1))
    }

    /// Returns the group that a packet carries as `wire`, masked to 4 bits.
    pub const fn from_wire(wire: u8) -> Self {
        Group(wire & 0xF)
    }

    /// Returns the number that a packet carries, 0 to 15.
    pub const fn wire(self) -> u8 {
        self.0
    }

    /// Returns the group's place among the 16, 0 to 15, for indexing a table.
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Display for Group {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", u16::from(self.0) + 1)
    }
}

/// A channel, carried as 0 to 15 and shown as 1 to 16, as musicians count.
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

    /// Returns the channel that a musician counts as `number`, 1 to 16.
    ///
    /// # Panics
    ///
    /// If `number` is not 1 to 16: in a `const`, at compile time.
    pub const fn number(number: u8) -> Self {
        assert!(number >= 1 && number <= 16, "channels are 1 to 16");
        Channel(number.saturating_sub(1))
    }

    /// Returns the channel that a packet carries as `wire`, masked to 4 bits.
    pub const fn from_wire(wire: u8) -> Self {
        Channel(wire & 0xF)
    }

    /// Returns the number that a packet carries, 0 to 15.
    pub const fn wire(self) -> u8 {
        self.0
    }

    /// Returns the channel's place among a group's 16, 0 to 15, for indexing
    /// a table.
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", u16::from(self.0) + 1)
    }
}

/// The stored sound that a program change selects, sent as 0 to 127 and shown
/// as 1 to 128 as General MIDI numbers them, so program 1 is the Acoustic
/// Grand Piano.
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
    /// Returns the program that a keyboard shows as `number`, 1 to 128.
    ///
    /// # Panics
    ///
    /// If `number` is not 1 to 128: in a `const`, at compile time.
    pub const fn number(number: u8) -> Self {
        assert!(number >= 1 && number <= 128, "programs are 1 to 128");
        Program(number.saturating_sub(1))
    }

    /// Returns the program that MIDI sends as `wire`, masked to 7 bits.
    pub const fn from_wire(wire: u8) -> Self {
        Program(wire & 0x7F)
    }

    /// Returns the number that MIDI sends, 0 to 127.
    pub const fn wire(self) -> u8 {
        self.0
    }
}

impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", u16::from(self.0) + 1)
    }
}

/// A value from -1 to 1 that rests at 0, such as a pitch bend, a brightness or
/// a pan.
///
/// MIDI 2.0 sends one as 32 bits, whose lowest value, centre (`0x8000_0000`)
/// and highest value are exactly -1, 0 and 1 here.
///
/// # Examples
///
/// ```
/// use hapto_midi::Centred;
///
/// assert_eq!(Centred::new(0.25).get(), 0.25);
/// // A value past either end is clamped to it.
/// assert_eq!(Centred::new(3.0).get(), 1.0);
/// assert_eq!(Centred::default(), Centred::CENTRE);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Centred(f32);

impl Centred {
    /// The resting value, 0.
    pub const CENTRE: Centred = Centred(0.0);

    /// Returns `value` clamped to -1 to 1, or the centre if `value` is NaN.
    pub fn new(value: f32) -> Self {
        Centred(held(value, -1.0, 1.0, 0.0))
    }

    /// Returns the value that MIDI 2.0's 32 bits carry, centred at
    /// `0x8000_0000`.
    pub(crate) fn from_bits(bits: u32) -> Self {
        const CENTRE: u32 = 0x8000_0000;
        let value = if bits >= CENTRE {
            f64::from(bits.saturating_sub(CENTRE)) / f64::from(u32::MAX - CENTRE)
        } else {
            -(f64::from(CENTRE.saturating_sub(bits)) / f64::from(CENTRE))
        };
        Centred(value as f32)
    }

    /// Returns the value, from -1 to 1.
    pub fn get(self) -> f32 {
        self.0
    }
}

impl fmt::Display for Centred {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:+.3}", self.0)
    }
}

/// The position of a damper, sostenuto or soft pedal, from 0 (up) to 1
/// (down).
///
/// [`is_down`](Self::is_down) reads it as a MIDI 1.0 switch, down from the
/// middle on, and [`depth`](Self::depth) returns the positions between that a
/// half-pedal keyboard sends.
///
/// # Examples
///
/// ```
/// use hapto_midi::Pedal;
///
/// // Half pedal is down as a MIDI 1.0 switch, and half way down.
/// let half = Pedal::new(0.5);
/// assert!(half.is_down());
/// assert_eq!(half.depth(), 0.5);
/// assert!(!Pedal::UP.is_down());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Pedal(f32);

impl Pedal {
    /// All the way up, with the dampers on the strings.
    pub const UP: Pedal = Pedal(0.0);
    /// All the way down.
    pub const DOWN: Pedal = Pedal(1.0);

    /// Returns a pedal `depth` down, clamped to 0 to 1, or up if `depth` is
    /// NaN.
    pub fn new(depth: f32) -> Self {
        Pedal(held(depth, 0.0, 1.0, 0.0))
    }

    /// Returns whether the pedal is down, from the middle on, as in MIDI 1.0.
    pub fn is_down(self) -> bool {
        self.0 >= 0.5
    }

    /// Returns how far down the pedal is, from 0 to 1.
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

/// A pitch as a key number with a fraction, so a quarter tone above A4 (69.0)
/// is 69.5.
///
/// # Examples
///
/// ```
/// use hapto_midi::{Key, Pitch};
///
/// assert_eq!(Pitch::from(Key::A4).hz(), 440.0);
/// assert!((Pitch::new(81.0).hz() - 880.0).abs() < 1e-3);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Pitch(f32);

impl Pitch {
    /// Returns the pitch `semitones` above key 0.
    pub const fn new(semitones: f32) -> Self {
        Pitch(semitones)
    }

    /// Returns the pitch in semitones above key 0.
    pub const fn semitones(self) -> f32 {
        self.0
    }

    /// Returns the frequency in Hz, with A4 at 440 Hz.
    pub fn hz(self) -> f32 {
        440.0 * ((self.0 - 69.0) / 12.0).exp2()
    }
}

/// The pitch of a key in equal temperament.
impl From<Key> for Pitch {
    fn from(key: Key) -> Self {
        Pitch(f32::from(key.0))
    }
}

impl fmt::Display for Pitch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3} semitones", self.0)
    }
}

/// A sounding note's id, from its note-on to its note-off.
///
/// It is the host's own id where the format has one, such as VST3's `noteId`.
/// Otherwise the decoder makes one up with the top bit set, so it never equals
/// a host's.
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

/// The bit that every made-up id sets, and no host's id does.
const MADE_UP: u32 = 0x8000_0000;
/// The bits that a plugin's own id sets, the made-up bit and the one below it,
/// which the decoder's ids never set.
const OWN: u32 = 0xC000_0000;

impl NoteId {
    /// Returns the host's id `id`, below `0x8000_0000`.
    pub const fn hosts(id: u32) -> Self {
        NoteId(id & !MADE_UP)
    }

    /// Returns an id of the plugin's own, `n` of up to 30 bits, for a note
    /// that it sends beside the ones it reads, such as a chord's other notes.
    ///
    /// It never equals a host's id or one that the decoder makes up, so its
    /// note goes out under no host id and a plugin can keep both in one table.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_midi::{Key, NoteId, NoteOn};
    ///
    /// // A fifth above the host's note, under the plugin's own id.
    /// let root = NoteOn::new(Key::new(60), 1.0).with_id(NoteId::hosts(7));
    /// let fifth = root.with_key(Key::new(67)).with_id(NoteId::own(1));
    /// assert!(!fifth.id.is_hosts());
    /// assert_ne!(fifth.id, root.id);
    /// ```
    pub const fn own(n: u32) -> Self {
        NoteId(n & !OWN | OWN)
    }

    /// Returns the `count`th made-up id, which wraps before it reaches a
    /// plugin's own ids.
    pub(crate) const fn made_up(count: u32) -> Self {
        NoteId(count & !OWN | MADE_UP)
    }

    /// Returns whether the host named the note, rather than Hapto.
    pub const fn is_hosts(self) -> bool {
        self.0 & MADE_UP == 0
    }

    /// Returns the id as one number, below `0x8000_0000` for a host's id and
    /// at or above it for Hapto's.
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

/// Returns `value` clamped to `low` to `high`, or `if_nan` if `value` is NaN.
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
