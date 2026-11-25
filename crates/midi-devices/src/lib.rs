//! The computer's MIDI devices as Universal MIDI Packets, for an app that
//! runs a plugin in place of a host.
//!
//! - [`Devices`] is the system's list, on the main thread. Each [`Device`]
//!   has a [`DeviceId`] that survives renumbering, and
//!   [`tick`](Devices::tick) returns what came and went.
//! - [`inputs`] makes the app's MIDI in. [`Inputs`] opens devices on the main
//!   thread, and [`InputsReader`] reads them all into each block on the
//!   audio thread, merged on group 1 as a host hands a plugin its MIDI, at
//!   frames timed by one [`Instant`] clock.
//! - [`outputs`] makes the app's MIDI out. [`Outputs`] opens devices on the
//!   main thread, and [`OutputsWriter`] hands each block's MIDI to them on
//!   the audio thread. Each device's own thread sends it as soon as the
//!   block has played, or each message at its frame's time, as [`Timing`]
//!   says.
//!
//! A device's messages pass between the system's thread and the audio thread
//! through a ring that never waits, locks or allocates, either way.
//!
//! The backends are the systems Hapto runs on: WinMM, MIDI 1.0 on Windows,
//! and ALSA's sequencer on Linux, MIDI 1.0 and, where the kernel and
//! alsa-lib have it, MIDI 2.0 in. Open: Windows MIDI Services and CoreMIDI;
//! on a system with no backend the list is empty and no input or output
//! opens.
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

// Miri cannot call into alsa-lib: under it, Linux has no backend.
#[cfg(all(target_os = "linux", not(miri)))]
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
#[cfg(not(any(windows, all(target_os = "linux", not(miri)))))]
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

#[cfg(all(target_os = "linux", not(miri)))]
use alsa as backend;
#[cfg(not(any(windows, all(target_os = "linux", not(miri)))))]
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
