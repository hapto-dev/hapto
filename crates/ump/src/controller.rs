//! The MIDI 1.0 controllers the translation between the protocols turns
//! on: bank select, which a program change carries in MIDI 2.0, and the
//! parameter-number controllers, which become one registered or assignable
//! controller.
//!
//! # Examples
//!
//! ```
//! use hapto_ump::controller;
//!
//! // Data entry and the address bytes are the upgrade's to assemble.
//! assert!(controller::is_parameter_number(controller::DATA_ENTRY));
//! assert!(controller::is_parameter_number(controller::NRPN_MSB));
//! // The mod wheel, controller 1, passes as it is.
//! assert!(!controller::is_parameter_number(1));
//! ```

/// Bank select's most significant byte.
pub const BANK_SELECT: u8 = 0;
/// Data entry's most significant byte: the high byte of an RPN's or NRPN's
/// value.
pub const DATA_ENTRY: u8 = 6;
/// Bank select's least significant byte.
pub const BANK_SELECT_LSB: u8 = 32;
/// Data entry's least significant byte: the low byte of an RPN's or NRPN's
/// value.
pub const DATA_ENTRY_LSB: u8 = 38;
/// Data increment, which steps the selected parameter up.
pub const DATA_INCREMENT: u8 = 96;
/// Data decrement, which steps it down.
pub const DATA_DECREMENT: u8 = 97;
/// An NRPN's least significant byte: its index.
pub const NRPN_LSB: u8 = 98;
/// An NRPN's most significant byte: its bank.
pub const NRPN_MSB: u8 = 99;
/// An RPN's least significant byte: its index.
pub const RPN_LSB: u8 = 100;
/// An RPN's most significant byte: its bank.
pub const RPN_MSB: u8 = 101;
/// Reset All Controllers, which also sets the parameter number to null
/// (RP-015).
pub const RESET_ALL_CONTROLLERS: u8 = 121;

/// Whether `controller` is one the upgrade consumes to assemble an RPN or
/// NRPN: data entry and its low byte, increment and decrement, and the four
/// address bytes.
///
/// # Examples
///
/// ```
/// use hapto_ump::controller::{RPN_LSB, is_parameter_number};
///
/// assert!(is_parameter_number(RPN_LSB));
/// // Bank select passes to the next program change instead.
/// assert!(!is_parameter_number(0));
/// ```
pub const fn is_parameter_number(controller: u8) -> bool {
    matches!(
        controller,
        DATA_ENTRY | DATA_ENTRY_LSB | DATA_INCREMENT..=RPN_MSB
    )
}
