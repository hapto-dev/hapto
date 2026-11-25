//! Why an input or an output did not open.

use std::error::Error;
use std::fmt;

/// Why a device did not open.
///
/// # Examples
///
/// ```
/// use hapto_midi_devices::{Devices, OpenError, inputs};
///
/// let (mut inputs, _reader) = inputs(64);
/// // An output cannot be read.
/// if let Some(synth) = Devices::new().outputs().next() {
///     assert_eq!(inputs.open(synth), Err(OpenError::NotAnInput));
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OpenError {
    /// The device only takes MIDI, so it can't be read.
    NotAnInput,
    /// The device only sends MIDI, so nothing can be sent to it.
    NotAnOutput,
    /// The device is not in the system's list now.
    Gone,
    /// Another app has the device open. WinMM allows one app at a time, and
    /// an ALSA port may accept only one subscriber.
    InUse,
    /// [`MAX_INPUTS`](crate::MAX_INPUTS) inputs, or
    /// [`MAX_OUTPUTS`](crate::MAX_OUTPUTS) outputs, are already open.
    NoRoom,
    /// The system ran out of memory, or ALSA's sequencer ran out of client
    /// numbers. It has 64 for all the apps on the system.
    NoMemory,
    /// No backend is built for this system.
    NoBackend,
    /// The system refused, with this error code.
    System(u32),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenError::NotAnInput => f.write_str("the device takes MIDI and sends none"),
            OpenError::NotAnOutput => f.write_str("the device sends MIDI and takes none"),
            OpenError::Gone => f.write_str("the device is not there now"),
            OpenError::InUse => f.write_str("another app holds the device"),
            OpenError::NoRoom => f.write_str("as many devices as may open are open already"),
            OpenError::NoMemory => f.write_str("the system had no memory for the device"),
            OpenError::NoBackend => f.write_str("Hapto reads no MIDI devices on this system yet"),
            OpenError::System(code) => write!(f, "the system refused the device (error {code})"),
        }
    }
}

impl Error for OpenError {}
