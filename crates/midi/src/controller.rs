//! MIDI 1.0's controllers, as MIDI 2.0 keeps them.
//!
//! Each is a [`Controller`] typed by its value, so a pan reads as
//! [`Centred`] and the damper as a [`Pedal`]. [`number`] has their numbers
//! for a match on a message, and [`name`] has their names.
//!
//! # Examples
//!
//! ```
//! use hapto_midi::{Centred, Decoder, Message, controller};
//! use hapto_ump::Event;
//!
//! // MIDI 2.0's pan, at rest.
//! let pan = Event::new(0, [0x40B0_0A00, 0x8000_0000, 0, 0]);
//! let mut decoder = Decoder::new();
//! let message = decoder.decode(&pan).next().expect("a controller");
//! assert_eq!(controller::PAN.read(&message), Some(Centred::CENTRE));
//! // Or matched by number, its value from 0 to 1.
//! assert!(matches!(
//!     message,
//!     Message::Control { controller: controller::number::PAN, value: 0.5, .. }
//! ));
//! assert_eq!(controller::name(controller::number::SUSTAIN), Some("Sustain Pedal"));
//! ```

use hapto_ump::scale::up_7_to_32;

use crate::message::{Message, NoteKey};
use crate::values::{Centred, Channel, Group, Pedal, held};

/// A controller, typed by its value, with the value that it rests at before
/// any message sets it.
///
/// It is a control change, or a registered or assignable controller (MIDI
/// 1.0's RPN and NRPN). MIDI 1.0's are in this module, and a profile's come
/// with the profile. A plugin defines its own, such as an assignable block
/// for its product, with its own value type if it needs one.
///
/// # Examples
///
/// ```
/// use hapto_midi::controller::{self, Controller};
/// use hapto_midi::{Channel, Group, Message, MidiOut, Pedal};
/// use hapto_ump::EventList;
///
/// // The plugin's cutoff, on assignable controller 1/7, half open.
/// const CUTOFF: Controller<f32> = Controller::assignable(1, 7, 0.5);
///
/// fn hear(message: &Message<'_>, cutoff: &mut f32, held: &mut bool) {
///     if let Some(value) = CUTOFF.read(message) {
///         *cutoff = value;
///     }
///     if let Some(pedal) = controller::SUSTAIN.read(message) {
///         *held = pedal.is_down();
///     }
/// }
///
/// let (mut cutoff, mut held) = (CUTOFF.default(), false);
/// hear(&controller::SUSTAIN.message(Group::FIRST, Channel::FIRST, Pedal::DOWN), &mut cutoff, &mut held);
/// assert!(held);
///
/// // Sending it writes its packets.
/// let mut sent = EventList::with_capacity(4);
/// let message = CUTOFF.message(Group::FIRST, Channel::FIRST, 0.25);
/// MidiOut::new(&mut sent).send(0, message);
/// assert_eq!(sent.as_slice().len(), 1);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Controller<T> {
    address: Address,
    default: T,
}

/// A [`Controller`]'s address in MIDI.
///
/// # Examples
///
/// ```
/// use hapto_midi::controller::{self, Address};
///
/// assert_eq!(controller::PAN.address(), Address::Control(10));
/// assert_eq!(
///     controller::FINE_TUNING.address(),
///     Address::Registered { bank: 0, index: 1 }
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Address {
    /// A control change, 0 to 127, and the registered per-note controller of
    /// the same number.
    Control(u8),
    /// A registered controller (MIDI 1.0's RPN).
    Registered {
        /// The bank, 0 to 127.
        bank: u8,
        /// The controller in its bank, 0 to 127.
        index: u8,
    },
    /// An assignable controller (MIDI 1.0's NRPN), which the instrument
    /// defines.
    Assignable {
        /// The bank, 0 to 127.
        bank: u8,
        /// The controller in its bank, 0 to 127.
        index: u8,
    },
}

/// A [`Controller`]'s value, which a message holds as a number from 0 to 1.
///
/// A plugin implements it for its own type, such as an enum of a
/// controller's positions.
///
/// # Examples
///
/// ```
/// use hapto_midi::controller::{Controller, ControllerValue};
///
/// /// A plugin's three-way switch.
/// #[derive(Debug, Clone, Copy, PartialEq)]
/// enum Mode {
///     Low,
///     Mid,
///     High,
/// }
///
/// impl ControllerValue for Mode {
///     fn from_unit(unit: f32) -> Self {
///         match unit {
///             u if u < 1.0 / 3.0 => Mode::Low,
///             u if u < 2.0 / 3.0 => Mode::Mid,
///             _ => Mode::High,
///         }
///     }
///
///     fn to_unit(self) -> f32 {
///         match self {
///             Mode::Low => 0.0,
///             Mode::Mid => 0.5,
///             Mode::High => 1.0,
///         }
///     }
/// }
///
/// const MODE: Controller<Mode> = Controller::control(20, Mode::Mid);
/// assert_eq!(Mode::from_unit(Mode::High.to_unit()), Mode::High);
/// assert_eq!(MODE.default(), Mode::Mid);
/// ```
pub trait ControllerValue: Copy {
    /// Returns the value that `unit`, from 0 to 1, stands for.
    fn from_unit(unit: f32) -> Self;

    /// Returns the number from 0 to 1 that stands for `self`.
    fn to_unit(self) -> f32;
}

/// From 0 to 1, as it comes.
impl ControllerValue for f32 {
    fn from_unit(unit: f32) -> Self {
        held(unit, 0.0, 1.0, 0.0)
    }

    fn to_unit(self) -> f32 {
        Self::from_unit(self)
    }
}

/// From -1 to 1, with 0 at MIDI 2.0's centre.
impl ControllerValue for Centred {
    fn from_unit(unit: f32) -> Self {
        // `Centred::new` returns the centre for NaN.
        Centred::new(unit.mul_add(2.0, -1.0))
    }

    fn to_unit(self) -> f32 {
        (self.get() + 1.0) / 2.0
    }
}

/// On from the middle up, like a MIDI 1.0 switch.
impl ControllerValue for bool {
    fn from_unit(unit: f32) -> Self {
        unit >= 0.5
    }

    fn to_unit(self) -> f32 {
        if self { 1.0 } else { 0.0 }
    }
}

/// Its depth, from 0 to 1.
impl ControllerValue for Pedal {
    fn from_unit(unit: f32) -> Self {
        Pedal::new(unit)
    }

    fn to_unit(self) -> f32 {
        self.depth()
    }
}

impl<T: Copy> Controller<T> {
    /// Returns control change `number`, 0 to 127, at rest at `default`.
    pub const fn control(number: u8, default: T) -> Self {
        Controller {
            address: Address::Control(number),
            default,
        }
    }

    /// Returns registered controller `index` of `bank`, at rest at
    /// `default`.
    ///
    /// # Panics
    ///
    /// For registered controller 0/0, the pitch bend range, because the
    /// decoder reads it as [`Message::BendRange`]. Read and send that message
    /// instead.
    pub const fn registered(bank: u8, index: u8, default: T) -> Self {
        assert!(
            bank != 0 || index != 0,
            "registered controller 0/0 is the pitch bend's range: read and send Message::BendRange"
        );
        Controller {
            address: Address::Registered { bank, index },
            default,
        }
    }

    /// Returns assignable controller `index` of `bank`, at rest at
    /// `default`.
    pub const fn assignable(bank: u8, index: u8, default: T) -> Self {
        Controller {
            address: Address::Assignable { bank, index },
            default,
        }
    }

    /// Returns its address in MIDI.
    pub const fn address(&self) -> Address {
        self.address
    }

    /// Returns its value before a message sets it, or after a channel resets
    /// it.
    pub const fn default(&self) -> T {
        self.default
    }
}

impl<T: ControllerValue> Controller<T> {
    /// Returns its value if `message` is a channel's message that sets it, on
    /// any group and channel.
    ///
    /// [`read_channel`](Self::read_channel) also returns the group and
    /// channel, and [`read_note`](Self::read_note) reads a per-note
    /// controller.
    pub fn read(&self, message: &Message<'_>) -> Option<T> {
        let (address, value) = message.controller()?;
        (address == self.address).then(|| T::from_unit(value))
    }

    /// Returns its value, with the message's group and channel, if `message`
    /// is a channel's message that sets it.
    ///
    /// It's for a plugin that keeps a value for each channel, such as in a
    /// [`PerChannel`](crate::PerChannel).
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_midi::controller::VOLUME;
    /// use hapto_midi::{Channel, Decoder, Group};
    /// use hapto_ump::Event;
    ///
    /// // Volume on channel 10, half way.
    /// let event = Event::new(0, [0x40B9_0700, 0x8000_0000, 0, 0]);
    /// let mut decoder = Decoder::new();
    /// let message = decoder.decode(&event).next().expect("a controller");
    /// let (group, channel, value) = VOLUME.read_channel(&message).expect("the volume");
    /// assert_eq!((group, channel), (Group::FIRST, Channel::number(10)));
    /// assert!((value - 0.5).abs() < 1e-6);
    /// ```
    pub fn read_channel(&self, message: &Message<'_>) -> Option<(Group, Channel, T)> {
        let value = self.read(message)?;
        Some((message.group(), message.channel()?, value))
    }

    /// Returns its value for one note, and the note, if `message` is the
    /// registered per-note controller of the same number, such as a key's
    /// brightness or pan.
    pub fn read_note(&self, message: &Message<'_>) -> Option<(NoteKey, T)> {
        match (self.address, message) {
            (
                Address::Control(number),
                Message::NoteControl {
                    note,
                    controller,
                    value,
                },
            ) if *controller == number => Some((*note, T::from_unit(*value))),
            _ => None,
        }
    }

    /// Returns the message that sets it to `value` on `group` and `channel`,
    /// for [`MidiOut::send`](crate::MidiOut::send).
    pub fn message(&self, group: Group, channel: Channel, value: T) -> Message<'static> {
        let value = value.to_unit();
        match self.address {
            Address::Control(controller) => Message::Control {
                group,
                channel,
                controller,
                value,
            },
            Address::Registered { bank, index } => Message::Registered {
                group,
                channel,
                bank,
                index,
                value,
            },
            Address::Assignable { bank, index } => Message::Assignable {
                group,
                channel,
                bank,
                index,
                value,
            },
        }
    }
}

/// Returns MIDI 1.0's 7-bit `value` from 0 to 1, upgraded as a device's
/// value is, so a default matches a keyboard's.
const fn sent(value: u8) -> f32 {
    (up_7_to_32(value) as f64 / u32::MAX as f64) as f32
}

/// The modulation wheel, from 0.
pub const MOD_WHEEL: Controller<f32> = Controller::control(number::MOD_WHEEL, 0.0);
/// The breath controller, from 0.
pub const BREATH: Controller<f32> = Controller::control(number::BREATH, 0.0);
/// The foot controller, from 0.
pub const FOOT: Controller<f32> = Controller::control(number::FOOT, 0.0);
/// The channel's volume, at General MIDI's 100 of 127.
pub const VOLUME: Controller<f32> = Controller::control(number::VOLUME, sent(100));
/// The balance between two sources, at the middle.
pub const BALANCE: Controller<Centred> = Controller::control(number::BALANCE, Centred::CENTRE);
/// Where the sound sits between the speakers, -1 left to 1 right, at the
/// middle.
pub const PAN: Controller<Centred> = Controller::control(number::PAN, Centred::CENTRE);
/// The expression pedal, a fraction of the volume, at full.
pub const EXPRESSION: Controller<f32> = Controller::control(number::EXPRESSION, 1.0);
/// The damper pedal, up.
pub const SUSTAIN: Controller<Pedal> = Controller::control(number::SUSTAIN, Pedal::UP);
/// The portamento switch, off.
pub const PORTAMENTO: Controller<bool> = Controller::control(number::PORTAMENTO, false);
/// The sostenuto pedal, up.
pub const SOSTENUTO: Controller<Pedal> = Controller::control(number::SOSTENUTO, Pedal::UP);
/// The soft pedal, up.
pub const SOFT: Controller<Pedal> = Controller::control(number::SOFT, Pedal::UP);
/// The legato footswitch, off.
pub const LEGATO: Controller<bool> = Controller::control(number::LEGATO, false);
/// The second hold pedal, off.
pub const HOLD_2: Controller<bool> = Controller::control(number::HOLD_2, false);
/// The fifth sound controller, brightness (MPE's timbre), at the middle.
pub const BRIGHTNESS: Controller<Centred> =
    Controller::control(number::BRIGHTNESS, Centred::CENTRE);
/// The channel's fine tuning (registered controller 1), a semitone either
/// way, at none.
pub const FINE_TUNING: Controller<Centred> = Controller::registered(0, 1, Centred::CENTRE);
/// The channel's coarse tuning (registered controller 2), from 64 semitones
/// down to 63 up, at none.
pub const COARSE_TUNING: Controller<Centred> = Controller::registered(0, 2, Centred::CENTRE);

/// MIDI 1.0's controllers by number, for a match on a message and for the
/// commands, such as all notes off.
///
/// # Examples
///
/// ```
/// use hapto_midi::Message;
/// use hapto_midi::controller::number;
///
/// fn stops_every_note(message: &Message<'_>) -> bool {
///     matches!(
///         message,
///         Message::Control { controller: number::ALL_NOTES_OFF | number::ALL_SOUND_OFF, .. }
///     )
/// }
/// # let _ = stops_every_note;
/// ```
pub mod number {
    /// The bank select high byte, which MIDI 2.0's program change carries.
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
    /// The expression pedal, a fraction of the volume.
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
    /// The second hold pedal.
    pub const HOLD_2: u8 = 69;
    /// The fifth sound controller, brightness (MPE's timbre).
    pub const BRIGHTNESS: u8 = 74;
    /// All sound off, at once.
    pub const ALL_SOUND_OFF: u8 = 120;
    /// Reset all controllers.
    pub const RESET_ALL_CONTROLLERS: u8 = 121;
    /// All notes off.
    pub const ALL_NOTES_OFF: u8 = 123;
}

/// Returns a controller's name in MIDI 1.0, or `None` if MIDI 1.0 leaves it
/// undefined.
///
/// Controllers 32 to 63 are the low bytes of 0 to 31.
///
/// # Examples
///
/// ```
/// use hapto_midi::controller::{self, number};
///
/// assert_eq!(controller::name(number::BRIGHTNESS), Some("Brightness"));
/// assert_eq!(controller::name(number::MOD_WHEEL + 32), Some("Modulation Wheel LSB"));
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
