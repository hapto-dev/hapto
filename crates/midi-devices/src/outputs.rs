//! The app's MIDI out: devices opened on the main thread, the audio
//! thread's writer that hands each block's MIDI to every one, and a thread
//! of each device's own that sends it at its time.

// Audio-path code: a panic here ends the app's audio, so indexing, unwrap,
// expect, panic and unchecked arithmetic are refused, and a site that is
// provably safe says why in an `#[expect]`.
#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects
    )
)]

use std::fmt;

use hapto_ump::{Bytes, Downgrade, Event};

use crate::Instant;
use crate::backend;
use crate::id::{Device, DeviceId, Direction};
use crate::inputs::OpenError;
use crate::ring::{Consumer, Producer, ring};

/// How many outputs may be open at once.
pub const MAX_OUTPUTS: usize = 16;

/// Makes the app's MIDI out, with room in each output for `room` packets
/// waiting to go, and splits it into its two sides: [`Outputs`], which opens
/// devices on the main thread, and [`OutputsWriter`], which hands each
/// block's MIDI to them on the audio thread.
///
/// Every output's room is made here, so opening a device later allocates
/// only on the main thread and the device's own.
///
/// # Examples
///
/// ```
/// use hapto_midi_devices::{Devices, Instant, outputs};
/// use hapto_ump::Event;
///
/// let (mut outputs, mut writer) = outputs(1024);
/// // On the main thread: a virtual port, by name.
/// for device in Devices::new().outputs() {
///     if device.name().starts_with("loopMIDI") {
///         let _ = outputs.open(device);
///     }
/// }
///
/// // On the audio thread, once per block: a MIDI 2.0 note-on at frame 0,
/// // which goes out as MIDI 1.0 when its time comes.
/// let sent = [Event::new(0, [0x4090_3C00, 0xFFFF_0000, 0, 0])];
/// writer.write(Instant::now(), 48_000.0, &sent);
/// ```
pub fn outputs(room: usize) -> (Outputs, OutputsWriter) {
    let (producers, consumers): (Vec<_>, Vec<_>) = (0..MAX_OUTPUTS).map(|_| ring(room)).unzip();
    let outputs = Outputs {
        slots: consumers.into_iter().map(Slot::Free).collect(),
    };
    let writer = OutputsWriter {
        rings: producers.into_boxed_slice(),
        lost: 0,
    };
    (outputs, writer)
}

/// The main thread's side of the app's MIDI out: the devices open now.
///
/// Each device gets every message the plugin sends, whatever its group: a
/// MIDI 1.0 device has none. A device that leaves while open is closed at
/// the next [`tick`](Self::tick). Every output closes as this drops.
///
/// Thread: main.
///
/// # Examples
///
/// ```
/// use hapto_midi_devices::{Devices, outputs};
///
/// let mut devices = Devices::new();
/// let (mut outputs, _writer) = outputs(1024);
///
/// // At each tick: follow the devices that come and go.
/// let changes = devices.tick(std::time::Instant::now());
/// for device in &changes.removed {
///     outputs.close(device.id());
/// }
/// for device in outputs.tick() {
///     println!("{} left", device.name());
/// }
/// assert_eq!(outputs.devices().count(), 0, "none opened");
/// ```
pub struct Outputs {
    slots: Vec<Slot>,
}

enum Slot {
    /// No device: the room waits for one.
    Free(Consumer),
    /// A device, sending from the room.
    Open(Device, backend::Output),
    /// A device the system would not close, whose room went with it.
    Lost,
}

impl fmt::Debug for Outputs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.devices()).finish()
    }
}

impl Outputs {
    /// Opens `device`. A device open already stays open.
    ///
    /// # Errors
    ///
    /// Returns why it did not open: an [`OpenError`].
    pub fn open(&mut self, device: &Device) -> Result<(), OpenError> {
        if device.direction() != Direction::Output {
            return Err(OpenError::NotAnOutput);
        }
        if self.is_open(device.id()) {
            return Ok(());
        }
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| matches!(slot, Slot::Free(_)))
            .ok_or(OpenError::NoRoom)?;
        let Slot::Free(mut consumer) = std::mem::replace(slot, Slot::Lost) else {
            return Err(OpenError::NoRoom);
        };
        // What a block pushed before the device came is not its to send.
        consumer.clear();
        match backend::Output::open(device, consumer) {
            Ok(output) => {
                *slot = Slot::Open(device.clone(), output);
                Ok(())
            }
            Err((why, consumer)) => {
                *slot = consumer.map_or(Slot::Lost, Slot::Free);
                Err(why)
            }
        }
    }

    /// Closes the device `id` names, if it is open, and says whether it was.
    /// What it had not sent yet is dropped with it.
    pub fn close(&mut self, id: &DeviceId) -> bool {
        let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| matches!(slot, Slot::Open(device, _) if device.id() == id))
        else {
            return false;
        };
        if let Slot::Open(_, output) = std::mem::replace(slot, Slot::Lost) {
            *slot = output.close().map_or(Slot::Lost, Slot::Free);
        }
        true
    }

    /// Whether the device `id` names is open.
    pub fn is_open(&self, id: &DeviceId) -> bool {
        self.devices().any(|device| device.id() == id)
    }

    /// The devices open now.
    pub fn devices(&self) -> impl Iterator<Item = &Device> {
        self.slots.iter().filter_map(|slot| match slot {
            Slot::Open(device, _) => Some(device),
            Slot::Free(_) | Slot::Lost => None,
        })
    }

    /// Closes the outputs whose device left while open, and returns them.
    /// Call it once per tick.
    pub fn tick(&mut self) -> Vec<Device> {
        let left: Vec<Device> = self
            .slots
            .iter()
            .filter_map(|slot| match slot {
                Slot::Open(device, output) if output.has_left() => Some(device.clone()),
                _ => None,
            })
            .collect();
        for device in &left {
            self.close(device.id());
        }
        left
    }

    /// Takes every packet waiting in output `output`'s room, as its device
    /// would, when no device is open there, and marks the room read, so
    /// the writer hands it the next blocks: `None` where a device is open.
    ///
    /// A hook for tests, which have no device to send to.
    #[doc(hidden)]
    pub fn take(&mut self, output: usize) -> Option<Vec<(Instant, [u32; 4])>> {
        let Some(Slot::Free(consumer)) = self.slots.get_mut(output) else {
            return None;
        };
        consumer.set_reading(true);
        let mut taken = Vec::new();
        while let Some(packet) = consumer.peek() {
            taken.push(packet);
            consumer.pop();
        }
        Some(taken)
    }
}

/// The audio thread's side of the app's MIDI out: it hands each block's
/// MIDI to every open output, each packet due at its frame's time.
///
/// The plugin's MIDI 2.0 is downgraded to MIDI 1.0 here, by the
/// specification's Default Translation, since every device on the systems
/// Hapto reads now takes MIDI 1.0 alone: what MIDI 1.0 has no message for
/// is dropped and counted in [`lost`](Self::lost). A packet goes to every
/// open output in the order the plugin sent it, and an output's thread
/// sends it once its time comes, at once if that has passed: never earlier,
/// and never dropped for being late.
///
/// # Examples
///
/// ```
/// use hapto_midi_devices::{Instant, outputs};
/// use hapto_ump::Event;
///
/// let (_outputs, mut writer) = outputs(1024);
/// // Each block, on the audio thread, once the plugin has played it: the
/// // block's first frame is due now, at 48 kHz.
/// let sent = [Event::new(480, [0x40B0_0700, u32::MAX, 0, 0])];
/// writer.write(Instant::now(), 48_000.0, &sent);
/// assert_eq!((writer.dropped(), writer.lost()), (0, 0));
/// ```
pub struct OutputsWriter {
    rings: Box<[Producer]>,
    lost: u64,
}

impl fmt::Debug for OutputsWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutputsWriter")
            .field("dropped", &self.dropped())
            .field("lost", &self.lost)
            .finish_non_exhaustive()
    }
}

impl OutputsWriter {
    /// Hands `sent`, a block's MIDI, to every open output: each packet due
    /// at `start`, when the block's first frame is, plus its frame at
    /// `rate` frames a second. Past an output's room a packet is dropped
    /// and counted in [`dropped`](Self::dropped).
    ///
    /// Thread: audio. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn write(&mut self, start: Instant, rate: f64, sent: &[Event]) {
        if !self.rings.iter().any(Producer::is_read) {
            return;
        }
        let nanos_per_frame = if rate > 0.0 { 1e9 / rate } else { 0.0 };
        let mut downgrade = Downgrade::new();
        let mut lost = 0_u64;
        let rings = &mut self.rings;
        for event in sent {
            let after = (f64::from(event.offset) * nanos_per_frame) as u64;
            let due = Instant::from_nanos(start.nanos().saturating_add(after));
            downgrade.translate(event, |packet| {
                // A packet MIDI 1.0's bytes cannot carry: a utility or
                // stream message, or 8-bit SysEx.
                if Bytes::of(packet.words).is_none() {
                    lost = lost.saturating_add(1);
                    return;
                }
                for ring in rings.iter_mut().filter(|ring| ring.is_read()) {
                    ring.push(due, packet.words);
                }
            });
        }
        self.lost = self
            .lost
            .saturating_add(lost)
            .saturating_add(downgrade.lost());
    }

    /// How many packets found no room in an open output's ring, or were a
    /// SysEx longer than its device sends whole, since it was made: the
    /// device sends too slowly for what the plugin sends.
    pub fn dropped(&self) -> u64 {
        self.rings.iter().map(Producer::dropped).sum()
    }

    /// How many messages MIDI 1.0 had no message for, since it was made.
    pub fn lost(&self) -> u64 {
        self.lost
    }
}

#[cfg(test)]
mod tests;
