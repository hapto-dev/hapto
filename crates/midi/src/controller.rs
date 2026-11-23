//! The controllers MIDI 1.0 defines, by number, as MIDI 2.0 keeps them,
//! with their names.
//!
//! # Examples
//!
//! ```
//! use hapto_midi::controller;
//!
//! assert_eq!(controller::name(controller::SUSTAIN), Some("Sustain Pedal"));
//! assert_eq!(controller::name(controller::MOD_WHEEL + 32), Some("Modulation Wheel LSB"));
//! // MIDI leaves controller 3 undefined.
//! assert_eq!(controller::name(3), None);
//! ```

/// Bank select's first half, which a program change carries in MIDI 2.0.
pub const BANK_SELECT: u8 = 0;
/// The modulation wheel.
pub const MOD_WHEEL: u8 = 1;
/// The breath controller.
pub const BREATH: u8 = 2;
/// The foot controller.
pub const FOOT: u8 = 4;
/// The channel's volume.
pub const VOLUME: u8 = 7;
/// The balance between two sources.
pub const BALANCE: u8 = 8;
/// Where the sound sits between the speakers.
pub const PAN: u8 = 10;
/// The expression pedal: a fraction of the volume.
pub const EXPRESSION: u8 = 11;
/// The damper pedal.
pub const SUSTAIN: u8 = 64;
/// The portamento switch.
pub const PORTAMENTO: u8 = 65;
/// The sostenuto pedal.
pub const SOSTENUTO: u8 = 66;
/// The soft pedal.
pub const SOFT: u8 = 67;
/// The legato footswitch.
pub const LEGATO: u8 = 68;
/// The fifth sound controller: brightness, MPE's timbre.
pub const BRIGHTNESS: u8 = 74;
/// All sound off, at once.
pub const ALL_SOUND_OFF: u8 = 120;
/// Reset all controllers.
pub const RESET_ALL_CONTROLLERS: u8 = 121;
/// All notes off.
pub const ALL_NOTES_OFF: u8 = 123;

/// The controller's name as MIDI 1.0 gives it, or `None` for one it leaves
/// undefined. Controllers 32 to 63 are the low halves of 0 to 31.
///
/// # Examples
///
/// ```
/// use hapto_midi::controller;
///
/// assert_eq!(controller::name(controller::BRIGHTNESS), Some("Brightness"));
/// assert_eq!(controller::name(200), None);
/// ```
pub fn name(number: u8) -> Option<&'static str> {
    Some(match number {
        0 => "Bank Select",
        1 => "Modulation Wheel",
        2 => "Breath Controller",
        4 => "Foot Controller",
        5 => "Portamento Time",
        6 => "Data Entry",
        7 => "Channel Volume",
        8 => "Balance",
        10 => "Pan",
        11 => "Expression",
        12 => "Effect Control 1",
        13 => "Effect Control 2",
        16 => "General Purpose 1",
        17 => "General Purpose 2",
        18 => "General Purpose 3",
        19 => "General Purpose 4",
        32 => "Bank Select LSB",
        33 => "Modulation Wheel LSB",
        34 => "Breath Controller LSB",
        36 => "Foot Controller LSB",
        37 => "Portamento Time LSB",
        38 => "Data Entry LSB",
        39 => "Channel Volume LSB",
        40 => "Balance LSB",
        42 => "Pan LSB",
        43 => "Expression LSB",
        44 => "Effect Control 1 LSB",
        45 => "Effect Control 2 LSB",
        48 => "General Purpose 1 LSB",
        49 => "General Purpose 2 LSB",
        50 => "General Purpose 3 LSB",
        51 => "General Purpose 4 LSB",
        64 => "Sustain Pedal",
        65 => "Portamento",
        66 => "Sostenuto",
        67 => "Soft Pedal",
        68 => "Legato Footswitch",
        69 => "Hold 2",
        70 => "Sound Variation",
        71 => "Timbre",
        72 => "Release Time",
        73 => "Attack Time",
        74 => "Brightness",
        75 => "Decay Time",
        76 => "Vibrato Rate",
        77 => "Vibrato Depth",
        78 => "Vibrato Delay",
        79 => "Sound Controller 10",
        80 => "General Purpose 5",
        81 => "General Purpose 6",
        82 => "General Purpose 7",
        83 => "General Purpose 8",
        84 => "Portamento Control",
        88 => "High Resolution Velocity Prefix",
        91 => "Reverb Depth",
        92 => "Tremolo Depth",
        93 => "Chorus Depth",
        94 => "Detune Depth",
        95 => "Phaser Depth",
        96 => "Data Increment",
        97 => "Data Decrement",
        98 => "NRPN LSB",
        99 => "NRPN MSB",
        100 => "RPN LSB",
        101 => "RPN MSB",
        120 => "All Sound Off",
        121 => "Reset All Controllers",
        122 => "Local Control",
        123 => "All Notes Off",
        124 => "Omni Mode Off",
        125 => "Omni Mode On",
        126 => "Mono Mode On",
        127 => "Poly Mode On",
        _ => return None,
    })
}
