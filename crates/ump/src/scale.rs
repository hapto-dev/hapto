//! Scales values between the widths of MIDI 1.0 and MIDI 2.0, by the Default
//! Translation in appendix D.1 of the UMP and MIDI 2.0 Protocol specification.
//!
//! Scaling up uses Min-Center-Max, which keeps the minimum, the centre and the
//! maximum. A value up to the centre is shifted left, and one above it also
//! fills its new low bits by repeating its own. Scaling down drops the low
//! bits, so a value scaled up and back down is unchanged.
//!
//! # Examples
//!
//! ```
//! use hapto_ump::scale;
//!
//! // A MIDI 1.0 controller's centre is the 32-bit centre.
//! assert_eq!(scale::up_7_to_32(64), 0x8000_0000);
//! // Scaling back down returns the original.
//! assert_eq!(scale::down_32_to_7(scale::up_7_to_32(87)), 87);
//! ```

/// Scales `value` from `from` bits up to `to` bits with Min-Center-Max.
/// `from` must be at least 2 and less than `to`, which must be at most 32.
const fn up(value: u32, from: u32, to: u32) -> u32 {
    let scale = to.saturating_sub(from);
    let shifted = value.wrapping_shl(scale);
    let centre = 1_u32.wrapping_shl(from.saturating_sub(1));
    if value <= centre {
        return shifted;
    }
    // Every bit except the highest is repeated until the new width is full.
    let repeat_bits = from.saturating_sub(1);
    let mask = 1_u32.wrapping_shl(repeat_bits).wrapping_sub(1);
    let mut repeat = value & mask;
    repeat = if scale > repeat_bits {
        repeat.wrapping_shl(scale.saturating_sub(repeat_bits))
    } else {
        repeat.wrapping_shr(repeat_bits.saturating_sub(scale))
    };
    let mut scaled = shifted;
    while repeat != 0 {
        scaled |= repeat;
        repeat = repeat.wrapping_shr(repeat_bits);
    }
    scaled
}

/// Scales a 7-bit value, such as a MIDI 1.0 velocity, up to 16 bits.
///
/// # Examples
///
/// ```
/// // The specification's own example.
/// assert_eq!(hapto_ump::scale::up_7_to_16(87), 0xAEBA);
/// ```
pub const fn up_7_to_16(value: u8) -> u16 {
    up(value as u32 & 0x7F, 7, 16) as u16
}

/// Scales a 7-bit value, such as a MIDI 1.0 controller, up to 32 bits.
///
/// # Examples
///
/// ```
/// assert_eq!(hapto_ump::scale::up_7_to_32(127), u32::MAX);
/// ```
pub const fn up_7_to_32(value: u8) -> u32 {
    up(value as u32 & 0x7F, 7, 32)
}

/// Scales a 14-bit value, such as a MIDI 1.0 pitch bend, up to 32 bits.
///
/// # Examples
///
/// ```
/// // A pitch bend at its centre stays at the centre.
/// assert_eq!(hapto_ump::scale::up_14_to_32(0x2000), 0x8000_0000);
/// ```
pub const fn up_14_to_32(value: u16) -> u32 {
    up(value as u32 & 0x3FFF, 14, 32)
}

/// Scales a 16-bit value, such as a MIDI 2.0 velocity, down to 7 bits.
///
/// # Examples
///
/// ```
/// assert_eq!(hapto_ump::scale::down_16_to_7(0xAEBA), 87);
/// ```
pub const fn down_16_to_7(value: u16) -> u8 {
    (value >> 9) as u8
}

/// Scales a 32-bit value, such as a MIDI 2.0 controller, down to 7 bits.
///
/// # Examples
///
/// ```
/// assert_eq!(hapto_ump::scale::down_32_to_7(0x8000_0000), 64);
/// ```
pub const fn down_32_to_7(value: u32) -> u8 {
    (value >> 25) as u8
}

/// Scales a 32-bit value, such as a MIDI 2.0 pitch bend, down to 14 bits.
///
/// # Examples
///
/// ```
/// assert_eq!(hapto_ump::scale::down_32_to_14(u32::MAX), 0x3FFF);
/// ```
pub const fn down_32_to_14(value: u32) -> u16 {
    (value >> 18) as u16
}

/// Scales a value from 0 to 1, such as a velocity, to MIDI 2.0's 16 bits.
/// The value is clamped to 0 to 1 and rounded, and NaN gives 0.
///
/// # Examples
///
/// ```
/// use hapto_ump::scale;
///
/// assert_eq!(scale::unit_to_16(1.0), u16::MAX);
/// assert_eq!(scale::unit_to_16(2.0), u16::MAX);
/// assert_eq!(scale::unit_of_16(scale::unit_to_16(0.25)), 16384.0 / 65535.0);
/// ```
pub fn unit_to_16(value: f64) -> u16 {
    (value.clamp(0.0, 1.0) * f64::from(u16::MAX)).round() as u16
}

/// Returns MIDI 2.0's 16 bits as a value from 0 to 1.
///
/// # Examples
///
/// ```
/// use hapto_ump::scale;
///
/// assert_eq!(scale::unit_of_16(u16::MAX), 1.0);
/// assert_eq!(scale::unit_of_16(0), 0.0);
/// ```
pub fn unit_of_16(bits: u16) -> f64 {
    f64::from(bits) / f64::from(u16::MAX)
}

/// Scales a value from 0 to 1, such as a controller's, to MIDI 2.0's 32
/// bits. The value is clamped to 0 to 1 and rounded, and NaN gives 0.
///
/// # Examples
///
/// ```
/// use hapto_ump::scale;
///
/// assert_eq!(scale::unit_to_32(1.0), u32::MAX);
/// assert_eq!(scale::unit_to_32(f64::NAN), 0);
/// assert_eq!(scale::unit_of_32(u32::MAX), 1.0);
/// ```
pub fn unit_to_32(value: f64) -> u32 {
    (value.clamp(0.0, 1.0) * f64::from(u32::MAX)).round() as u32
}

/// Returns MIDI 2.0's 32 bits as a value from 0 to 1.
///
/// # Examples
///
/// ```
/// use hapto_ump::scale;
///
/// // A controller at its 32-bit centre is just past half.
/// assert!(scale::unit_of_32(0x8000_0000) > 0.5);
/// ```
pub fn unit_of_32(bits: u32) -> f64 {
    f64::from(bits) / f64::from(u32::MAX)
}

/// Returns a pitch in semitones, with key 0 at 0, as the 7.25 fixed point of
/// a note's absolute pitch controller,
/// [`ChannelMessage::PER_NOTE_PITCH`](crate::ChannelMessage::PER_NOTE_PITCH).
/// The pitch is clamped to 0 to just under 128.
///
/// # Examples
///
/// ```
/// use hapto_ump::scale;
///
/// // A4, 69, a quarter tone sharp.
/// let bits = scale::pitch_7_25(69.5);
/// assert_eq!(bits, 69 << 25 | 1 << 24);
/// assert_eq!(scale::semitones_7_25(bits), 69.5);
/// ```
pub fn pitch_7_25(semitones: f64) -> u32 {
    (semitones.clamp(0.0, 127.999_999) * f64::from(1_u32 << 25)).round() as u32
}

/// Returns a note's absolute pitch, in 7.25 fixed point, as semitones.
///
/// # Examples
///
/// ```
/// use hapto_ump::scale;
///
/// // Middle C, 60, and a quarter of a semitone.
/// assert_eq!(scale::semitones_7_25(60 << 25 | 1 << 23), 60.25);
/// ```
pub fn semitones_7_25(bits: u32) -> f64 {
    f64::from(bits) / f64::from(1_u32 << 25)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_specifications_examples_scale_as_it_says() {
        // The specification's examples, from appendix D.1.3 and D.1.4.
        let examples = [(10, 0x1400), (64, 0x8000), (87, 0xAEBA), (127, 0xFFFF)];
        for (seven, sixteen) in examples {
            assert_eq!(up_7_to_16(seven), sixteen, "{seven} up");
            assert_eq!(down_16_to_7(sixteen), seven, "{sixteen:#x} down");
        }
        // Appendix D.3.1's example, a MIDI 1.0 velocity of 1.
        assert_eq!(up_7_to_16(1), 0x0200);
    }

    #[test]
    fn minimum_centre_and_maximum_land_on_their_own() {
        assert_eq!(
            [up_7_to_32(0), up_7_to_32(64), up_7_to_32(127)],
            [0, 0x8000_0000, u32::MAX]
        );
        assert_eq!(
            [up_14_to_32(0), up_14_to_32(0x2000), up_14_to_32(0x3FFF)],
            [0, 0x8000_0000, u32::MAX]
        );
        assert_eq!(
            [up_7_to_16(0), up_7_to_16(64), up_7_to_16(127)],
            [0, 0x8000, u16::MAX]
        );
    }

    #[test]
    fn above_the_centre_the_bits_repeat_to_32_bits_as_the_reference_code_does() {
        // Values from appendix D.1.3's reference code, not from this code:
        // the bits below the top one repeat through the whole word.
        assert_eq!(up_7_to_32(65), 0x8208_2082);
        assert_eq!(up_7_to_32(87), 0xAEBA_EBAE);
        assert_eq!(up_14_to_32(0x2001), 0x8004_0020);
    }

    #[test]
    fn every_value_comes_back_down_as_it_went_up() {
        for value in 0..=127 {
            assert_eq!(down_16_to_7(up_7_to_16(value)), value);
            assert_eq!(down_32_to_7(up_7_to_32(value)), value);
        }
        for value in 0..=0x3FFF {
            assert_eq!(down_32_to_14(up_14_to_32(value)), value);
        }
    }

    #[test]
    fn values_only_ever_rise() {
        for value in 1..=127 {
            assert!(up_7_to_16(value) > up_7_to_16(value - 1));
            assert!(up_7_to_32(value) > up_7_to_32(value - 1));
        }
        for value in 1..=0x3FFF {
            assert!(up_14_to_32(value) > up_14_to_32(value - 1));
        }
    }
}
