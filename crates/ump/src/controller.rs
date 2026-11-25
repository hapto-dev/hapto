//! The MIDI 1.0 controllers the translation between the protocols turns
//! on: bank select, which a program change carries in MIDI 2.0, and the
//! parameter-number controllers, which become one registered or assignable
//! controller. The ones an exporter ranks are public; the rest serve the
//! translation alone.
//!
//! # Examples
//!
//! ```
//! use hapto_ump::controller;
//!
//! // Data entry and the address bytes are the upgrade's to assemble.
//! assert!(controller::is_parameter_number(controller::DATA_ENTRY));
//! assert!(controller::is_parameter_number(controller::RPN_MSB));
//! // The mod wheel, controller 1, passes as it is.
//! assert!(!controller::is_parameter_number(1));
//! ```

/// Bank select's most significant byte.
pub(crate) const BANK_SELECT: u8 = 0;
/// Data entry's most significant byte, the high byte of an RPN or NRPN value.
pub const DATA_ENTRY: u8 = 6;
/// Bank select's least significant byte.
pub(crate) const BANK_SELECT_LSB: u8 = 32;
/// Data entry's least significant byte, the low byte of an RPN or NRPN value.
pub(crate) const DATA_ENTRY_LSB: u8 = 38;
/// Data increment, which steps the selected parameter up.
pub(crate) const DATA_INCREMENT: u8 = 96;
/// Data decrement, which steps the selected parameter down.
pub(crate) const DATA_DECREMENT: u8 = 97;
/// An NRPN's least significant byte, its index.
pub const NRPN_LSB: u8 = 98;
/// An NRPN's most significant byte, its bank.
pub(crate) const NRPN_MSB: u8 = 99;
/// An RPN's least significant byte, its index.
pub(crate) const RPN_LSB: u8 = 100;
/// An RPN's most significant byte, its bank.
pub const RPN_MSB: u8 = 101;
/// Reset All Controllers, which also sets the parameter number to null
/// (RP-015).
pub(crate) const RESET_ALL_CONTROLLERS: u8 = 121;

/// Whether `controller` is one of MIDI 1.0's parameter-number controllers:
/// data entry, its low byte and the four address bytes, which the upgrade
/// assembles into an RPN or NRPN, and increment and decrement, which step
/// one.
///
/// # Examples
///
/// ```
/// use hapto_ump::controller::{NRPN_LSB, is_parameter_number};
///
/// assert!(is_parameter_number(NRPN_LSB));
/// // Bank select passes to the next program change instead.
/// assert!(!is_parameter_number(0));
/// ```
pub const fn is_parameter_number(controller: u8) -> bool {
    matches!(
        controller,
        DATA_ENTRY | DATA_ENTRY_LSB | DATA_INCREMENT..=RPN_MSB
    )
}
