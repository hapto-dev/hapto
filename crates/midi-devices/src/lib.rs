//! The computer's MIDI devices as Universal MIDI Packets, for an app that
//! runs a plugin with no host and reads them where a host would.
//!
//! [`Devices`] is the system's list, on the main thread: each [`Device`] is
//! named by a [`DeviceId`] that survives the system's renumbering, and a
//! [`tick`](Devices::tick) tells what came and went. [`inputs`] makes the
//! app's MIDI in: [`Inputs`] opens devices on the main thread, and
//! [`InputsReader`] reads every one into each block on the audio thread,
//! merged on group 1 as a host hands a plugin its MIDI, at frames, stamped on
//! one [`Instant`] clock. [`outputs`] makes the app's MIDI out: [`Outputs`]
//! opens devices on the main thread, and [`OutputsWriter`] hands each block's
//! MIDI to every one on the audio thread, each message due at its frame's
//! time, which a thread of the device's own sends it at. A device's messages
//! travel between the system's thread and the audio thread through a ring
//! that never waits, locks or allocates, either way.
//!
//! A device speaks UMP here whatever the system's API carries: a MIDI 1.0
//! device's bytes become packets, upgraded to MIDI 2.0 with a translation of
//! the device's own, and what goes out to one is downgraded to MIDI 1.0. Only
//! an app links this crate, so a plugin in a host never sees a device.
//!
//! The backends are the systems Hapto runs on: WinMM, MIDI 1.0 on Windows.
//! Open: Windows MIDI Services, CoreMIDI and ALSA; on a system with no
//! backend the list is empty and no input or output opens.
//!
//! # Examples
//!
//! Every input the computer has, read into one block:
//!
//! ```
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

mod clock;
mod devices;
mod id;
mod inputs;
#[cfg(not(windows))]
mod none;
mod outputs;
mod ring;
#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "a system boundary: every block says why it holds"
)]
mod winmm;

#[cfg(not(windows))]
use none as backend;
#[cfg(windows)]
use winmm as backend;

pub use clock::Instant;
pub use devices::{Changes, Devices};
pub use id::{Device, DeviceId, Direction};
pub use inputs::{Inputs, InputsReader, MAX_INPUTS, OpenError, inputs};
pub use outputs::{MAX_OUTPUTS, Outputs, OutputsWriter, outputs};
