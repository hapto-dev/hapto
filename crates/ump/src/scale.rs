//! Values between the widths MIDI 1.0 and MIDI 2.0 send them at, by the
//! specification's Default Translation (M2-104-UM, appendix D.1).
//!
//! Upscaling is Min-Center-Max: the minimum, the centre and the maximum land
//! on the minimum, the centre and the maximum, values up to the centre shift
//! left, and values above it fill their low bits by repeating their own.
//! Downscaling cuts the low bits off. Down after up gives back every value
//! exactly, so a value upgraded from MIDI 1.0 narrows back to what was sent.
//!
//! # Examples
//!
//! ```
//! use hapto_ump::scale;
//!
//! // A MIDI 1.0 controller at its centre is the 32-bit centre.
//! assert_eq!(scale::up_7_to_32(64), 0x8000_0000);
//! // And narrows back to what was sent.
//! assert_eq!(scale::down_32_to_7(scale::up_7_to_32(87)), 87);
//! ```

/// `value`, `from` bits wide, upscaled to `to` bits by Min-Center-Max, the
/// specification's reference algorithm. `from` is 2 or more and less than
/// `to`, which is at most 32.
const fn up(value: u32, from: u32, to: u32) -> u32 {
    let scale = to.saturating_sub(from);
    let shifted = value.wrapping_shl(scale);
    let centre = 1_u32.wrapping_shl(from.saturating_sub(1));
    if value <= centre {
        return shifted;
    }
    // Every bit but the highest repeats, as often as the new width holds.
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

/// A 7-bit value, such as a MIDI 1.0 velocity, as 16 bits.
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

/// A 7-bit value, such as a MIDI 1.0 controller, as 32 bits.
///
/// # Examples
///
/// ```
/// assert_eq!(hapto_ump::scale::up_7_to_32(127), u32::MAX);
/// ```
pub const fn up_7_to_32(value: u8) -> u32 {
    up(value as u32 & 0x7F, 7, 32)
}

/// A 14-bit value, such as a MIDI 1.0 pitch bend or parameter number's
/// data, as 32 bits.
///
/// # Examples
///
/// ```
/// // A pitch bend at rest stays at rest.
/// assert_eq!(hapto_ump::scale::up_14_to_32(0x2000), 0x8000_0000);
/// ```
pub const fn up_14_to_32(value: u16) -> u32 {
    up(value as u32 & 0x3FFF, 14, 32)
}

/// A 16-bit value, such as a MIDI 2.0 velocity, cut to 7 bits.
///
/// # Examples
///
/// ```
/// assert_eq!(hapto_ump::scale::down_16_to_7(0xAEBA), 87);
/// ```
pub const fn down_16_to_7(value: u16) -> u8 {
    (value >> 9) as u8
}

/// A 32-bit value, such as a MIDI 2.0 controller, cut to 7 bits.
///
/// # Examples
///
/// ```
/// assert_eq!(hapto_ump::scale::down_32_to_7(0x8000_0000), 64);
/// ```
pub const fn down_32_to_7(value: u32) -> u8 {
    (value >> 25) as u8
}

/// A 32-bit value, such as a MIDI 2.0 pitch bend, cut to 14 bits.
///
/// # Examples
///
/// ```
/// assert_eq!(hapto_ump::scale::down_32_to_14(u32::MAX), 0x3FFF);
/// ```
pub const fn down_32_to_14(value: u32) -> u16 {
    (value >> 18) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_specifications_examples_scale_as_it_says() {
        // M2-104-UM, D.1.3 and D.1.4.
        let examples = [(10, 0x1400), (64, 0x8000), (87, 0xAEBA), (127, 0xFFFF)];
        for (seven, sixteen) in examples {
            assert_eq!(up_7_to_16(seven), sixteen, "{seven} up");
            assert_eq!(down_16_to_7(sixteen), seven, "{sixteen:#x} down");
        }
        // D.3.1: a MIDI 1.0 velocity of 1.
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
