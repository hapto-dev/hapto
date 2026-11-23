//! The computer's MIDI devices as Universal MIDI Packets, for an app that
//! runs a plugin with no host and reads them where a host would.
//!
//! [`Devices`] is the system's list, on the main thread: each [`Device`] is
//! named by a [`DeviceId`] that survives the system's renumbering, and a
//! [`tick`](Devices::tick) tells what came and went. [`inputs`] makes the
//! app's MIDI in: [`Inputs`] opens devices on the main thread, each on a UMP
//! group of its own, and [`InputsReader`] reads every one into each block on
//! the audio thread, at frames, stamped on one [`Instant`] clock. A device's
//! messages travel from the system's thread to the audio thread through a
//! ring that never waits, locks or allocates.
//!
//! A device speaks UMP here whatever the system's API carries: a MIDI 1.0
//! device's bytes become MIDI 1.0 packets on its group, for the engine to
//! upgrade. Only an app links this crate, so a plugin in a host never sees a
//! device.
//!
//! The backends are the systems Hapto runs on: WinMM, MIDI 1.0 on Windows.
//! Open: Windows MIDI Services, CoreMIDI and ALSA, and output to a device;
//! on a system with no backend the list is empty and no input opens.
//!
//! # Examples
//!
//! Every input the computer has, read into one block:
//!
//! ```
//! use hapto_devices::{Devices, Instant, inputs};
//! use hapto_ump::EventList;
//!
//! let devices = Devices::new();
//! let (mut inputs, mut reader) = inputs(1024);
//! for device in devices.inputs() {
//!     match inputs.open(device) {
//!         Ok(group) => println!("{} on group {}", device.name(), group + 1),
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
pub use inputs::{GROUPS, Inputs, InputsReader, OpenError, inputs};
