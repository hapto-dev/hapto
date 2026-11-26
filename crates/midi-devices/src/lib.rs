//! The computer's MIDI devices as Universal MIDI Packets, for an app that
//! runs a plugin in place of a host.
//!
//! - [`Devices`] is the system's list, on the main thread. Each [`Device`]
//!   has a [`DeviceId`] that survives renumbering, and
//!   [`tick`](Devices::tick) returns what came and went.
//! - [`inputs`] makes the app's MIDI in. [`Inputs`] opens devices on the main
//!   thread, and [`InputsReader`] reads them all into each block on the
//!   audio thread, at frames timed by one [`Instant`] clock. Every MIDI 1.0
//!   device is merged on group 1, as a host hands a plugin its MIDI.
//! - [`outputs`] makes the app's MIDI out. [`Outputs`] opens devices on the
//!   main thread, and [`OutputsWriter`] hands each block's MIDI to them on
//!   the audio thread. Each device's own thread sends it as soon as the
//!   block has played, or each message at its frame's time, as [`Timing`]
//!   says.
//!
//! A device's messages pass between the system's thread and the audio thread
//! through a ring that never waits, locks or allocates, either way.
//!
//! Every device reads and writes UMP here, whatever the system's API carries.
//! A MIDI 1.0 device's bytes become packets, upgraded to MIDI 2.0 by the
//! device's own translation, and what goes out to one is downgraded to MIDI
//! 1.0. Only an app links this crate, so a plugin in a host never sees a
//! device.
//!
//! The backends are WinMM on Windows, which carries only MIDI 1.0, and ALSA's
//! sequencer on Linux, which carries MIDI 1.0 both ways and MIDI 2.0 input
//! where the kernel and alsa-lib support UMP. Elsewhere the list is empty and
//! no input or output opens.
//!
//! Open: backends for Windows MIDI Services and CoreMIDI.
//!
//! # Examples
//!
//! Every input that the computer has, read into one block:
//!
//! ```no_run
//! use hapto_midi_devices::{Devices, Instant, inputs};
//! use hapto_ump::EventList;
//!
//! let devices = Devices::new();
//! let (mut inputs, mut reader) = inputs(1024);
//! for device in devices.inputs() {
//!     match inputs.open(device) {
//!         Ok(()) => println!("MIDI in from {}", device.name()),
//!         Err(why) => println!("{}: {why}", device.name()),
//!     }
//! }
//! let mut block = EventList::with_capacity(2048);
//! reader.read(Instant::now(), 512, &mut block);
//! ```

// The nightly real-time sanitizer lane marks the audio thread's calls.
#![cfg_attr(hapto_rtsan, feature(sanitize))]

// Each backend builds where build.rs sets `hapto_backend`, and `none`
// builds elsewhere.
#[cfg(all(target_os = "linux", hapto_backend))]
#[allow(
    unsafe_code,
    reason = "a system boundary: every block says why it holds"
)]
mod alsa;
mod clock;
mod devices;
mod error;
mod id;
mod inputs;
#[cfg(not(hapto_backend))]
mod none;
mod outputs;
mod ports;
mod ring;
#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "a system boundary: every block says why it holds"
)]
mod winmm;

#[cfg(all(target_os = "linux", hapto_backend))]
use alsa as backend;
#[cfg(not(hapto_backend))]
use none as backend;
#[cfg(windows)]
use winmm as backend;

pub use clock::Instant;
pub use devices::{Changes, Devices};
pub use error::OpenError;
pub use id::{Device, DeviceId, Direction};
pub use inputs::{Inputs, InputsReader, MAX_INPUTS, inputs};
pub use outputs::{MAX_OUTPUTS, Outputs, OutputsWriter, Timing, outputs};
pub use ports::OpenDevices;
