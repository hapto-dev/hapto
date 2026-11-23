//! The pitch each key plays before anything bends it.

use crate::values::{Key, Pitch};

/// The pitch each key starts from: equal temperament unless a plugin sets a
/// scale of its own. A note-on carrying a pitch of its own, MIDI 2.0's pitch
/// attribute, plays that instead, and a note's own pitch message overrides
/// both.
///
/// Open: the MIDI Tuning Standard over SysEx, and the Piano Profile's
/// temperament, are to come.
///
/// # Examples
///
/// ```
/// use hapto_midi::{Key, Pitch, Tuning};
///
/// // A4 at 432 Hz: every key a third of a semitone flat.
/// let offset = 12.0 * (432.0_f32 / 440.0).log2();
/// let tuning = Tuning::from_fn(|key| Pitch::new(f32::from(key.wire()) + offset));
/// assert!((tuning.pitch(Key::A4).hz() - 432.0).abs() < 0.01);
/// assert_eq!(Tuning::equal().pitch(Key::A4).hz(), 440.0);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Tuning {
    pitches: [Pitch; 128],
}

impl Tuning {
    /// Equal temperament, A4 at 440 Hz: each key's pitch is its number.
    pub fn equal() -> Self {
        Tuning::from_fn(Pitch::from)
    }

    /// The tuning that plays `pitch(key)` at each key.
    pub fn from_fn(mut pitch: impl FnMut(Key) -> Pitch) -> Self {
        let mut key = 0_u8;
        Tuning {
            pitches: std::array::from_fn(|_| {
                let at = pitch(Key::new(key));
                key = key.wrapping_add(1);
                at
            }),
        }
    }

    /// The pitch `key` starts from.
    pub fn pitch(&self, key: Key) -> Pitch {
        self.pitches
            .get(usize::from(key.wire()))
            .copied()
            .unwrap_or(Pitch::from(key))
    }
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning::equal()
    }
}
