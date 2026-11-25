//! The app's MIDI out, with devices opened on the main thread and each
//! block's MIDI handed to them on the audio thread.
//!
//! Each device's own thread sends the MIDI as soon as the block has played,
//! or each message at its frame's time.

// A panic on the audio path ends the app's audio, so indexing, unwrap,
// expect, panic and unchecked arithmetic are denied. A provably safe site
// gives its reason in an `#[expect]`.
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
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use hapto_ump::{Downgrade, Event};

use crate::Instant;
use crate::backend;
use crate::error::OpenError;
use crate::id::{Device, DeviceId, Direction};
use crate::ports::{OpenDevices, Port, Ports};
use crate::ring::{Consumer, Producer, ring};

// A system with no backend sends no SysEx.
#[cfg(any(windows, all(target_os = "linux", not(miri))))]
mod sysex;

#[cfg(any(windows, all(target_os = "linux", not(miri))))]
pub(crate) use sysex::Sysex;

/// How many outputs may be open at once.
pub const MAX_OUTPUTS: usize = 16;

/// When an output sends a block's MIDI.
///
/// A message from an input is placed in a block by the time it came, so it
/// plays a block late but keeps its timing within the block.
///
/// - [`Immediate`](Self::Immediate) sends the block's MIDI as soon as the
///   block has played, as JUCE's standalone does. It has the least latency,
///   from none to a block after the input, but each message goes up to a
///   block early, so the block's spacing is lost.
/// - [`AtFrames`](Self::AtFrames) sends each message at its frame's time, so
///   an arpeggio stays even and a clock steady, and a message from an input
///   always goes out a block after it came.
///
/// # Examples
///
/// ```
/// use hapto_midi_devices::{Timing, outputs};
///
/// let (outputs, _writer) = outputs(1024);
/// assert_eq!(outputs.timing(), Timing::Immediate);
/// // A sequencer's notes keep their spacing.
/// outputs.set_timing(Timing::AtFrames);
/// assert_eq!(outputs.timing(), Timing::AtFrames);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Timing {
    /// As soon as the block has played, for the least latency.
    #[default]
    Immediate,
    /// Each message at its frame's time, keeping the block's spacing.
    AtFrames,
}

impl Timing {
    const fn index(self) -> u8 {
        match self {
            Timing::Immediate => 0,
            Timing::AtFrames => 1,
        }
    }

    const fn from_index(index: u8) -> Timing {
        match index {
            1 => Timing::AtFrames,
            _ => Timing::Immediate,
        }
    }
}

/// Makes the app's MIDI out, with room in each output for `room` packets
/// waiting to go, and splits it into [`Outputs`] for the main thread and
/// [`OutputsWriter`] for the audio thread.
///
/// Every output's room is allocated here, so opening a device later
/// allocates only on the main thread and the device's own thread.
///
/// # Examples
///
/// ```no_run
/// use hapto_midi_devices::{Devices, Instant, outputs};
/// use hapto_ump::Event;
///
/// let (mut outputs, mut writer) = outputs(1024);
/// // On the main thread, a virtual port found by name.
/// for device in Devices::new().outputs() {
///     if device.name().starts_with("loopMIDI") {
///         let _ = outputs.open(device);
///     }
/// }
///
/// // On the audio thread, once per block, a MIDI 2.0 note-on at frame 0,
/// // which goes out as MIDI 1.0 when its time comes.
/// let sent = [Event::new(0, [0x4090_3C00, 0xFFFF_0000, 0, 0])];
/// writer.write(Instant::now(), 48_000.0, &sent);
/// ```
pub fn outputs(room: usize) -> (Outputs, OutputsWriter) {
    let (producers, consumers): (Vec<_>, Vec<_>) = (0..MAX_OUTPUTS).map(|_| ring(room)).unzip();
    let signals: Vec<Arc<backend::Signal>> = (0..MAX_OUTPUTS)
        .map(|_| Arc::new(backend::Signal::new()))
        .collect();
    let timing = Arc::new(AtomicU8::new(Timing::default().index()));
    let outputs = Outputs {
        ports: Ports::new(consumers),
        signals: signals.iter().map(Arc::clone).collect(),
        timing: Arc::clone(&timing),
    };
    let writer = OutputsWriter {
        rings: producers.into_iter().zip(signals).collect(),
        timing,
        lost: 0,
    };
    (outputs, writer)
}

/// The main thread's side of the app's MIDI out, with the open devices.
///
/// Each device gets every message the plugin sends, whatever its group,
/// because a MIDI 1.0 device has no groups.
///
/// - A device found gone when the output sends to it closes at the next
///   [`tick`](Self::tick).
/// - One that leaves while nothing is sent closes once the system's list
///   drops it.
/// - Every output closes when this drops.
///
/// Open: with Windows 10's WDM driver, unplugging a device during a long
/// SysEx leaves the send stuck in the driver for good, and every later read
/// of the device list waits behind it, so a tick hangs. Short messages are
/// safe, and JUCE hangs the same way.
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
/// // At each tick, follow the devices that come and go.
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
    ports: Ports<backend::Output>,
    /// The signal that wakes each output's thread, set by the writer.
    signals: Box<[Arc<backend::Signal>]>,
    /// When the outputs send, shared with the writer.
    timing: Arc<AtomicU8>,
}

impl Port for backend::Output {
    type Room = Consumer;

    fn has_left(&self) -> bool {
        backend::Output::has_left(self)
    }

    fn close(self) -> Option<Consumer> {
        backend::Output::close(self)
    }
}

impl fmt::Debug for Outputs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.devices()).finish()
    }
}

impl Outputs {
    /// Opens `device`, unless it is already open.
    ///
    /// # Errors
    ///
    /// Returns an [`OpenError`] if the device did not open.
    pub fn open(&mut self, device: &Device) -> Result<(), OpenError> {
        if device.direction() != Direction::Output {
            return Err(OpenError::NotAnOutput);
        }
        let signals = &self.signals;
        self.ports.open(device, |place, mut consumer| {
            let Some(signal) = signals.get(place) else {
                return Err((OpenError::NoRoom, Some(consumer)));
            };
            // A device doesn't send what a block pushed before it came.
            consumer.clear();
            backend::Output::open(device, consumer, Arc::clone(signal))
        })
    }

    /// Closes the device that `id` names, if it is open, dropping what it
    /// has not sent, and returns whether it was open.
    pub fn close(&mut self, id: &DeviceId) -> bool {
        self.ports.close(id)
    }

    /// Returns whether the device that `id` names is open.
    pub fn is_open(&self, id: &DeviceId) -> bool {
        self.ports.is_open(id)
    }

    /// Sets every output's timing to `timing`, from the next block on.
    pub fn set_timing(&self, timing: Timing) {
        self.timing.store(timing.index(), Ordering::Relaxed);
    }

    /// Returns when the outputs send a block's MIDI.
    pub fn timing(&self) -> Timing {
        Timing::from_index(self.timing.load(Ordering::Relaxed))
    }

    /// Returns the open devices.
    pub fn devices(&self) -> impl Iterator<Item = &Device> {
        self.ports.devices()
    }

    /// Closes and returns the devices that left while open.
    ///
    /// Call it once per tick.
    pub fn tick(&mut self) -> Vec<Device> {
        self.ports.tick()
    }

    /// Takes every packet waiting in output `output`'s room, as its device
    /// would, and marks the room read so the writer hands it the next blocks,
    /// or returns `None` if a device is open there.
    ///
    /// A hook for tests, which have no device to send to.
    #[doc(hidden)]
    pub fn take(&mut self, output: usize) -> Option<Vec<(Instant, [u32; 4])>> {
        let consumer = self.ports.free_mut(output)?;
        consumer.set_reading(true);
        let mut taken = Vec::new();
        while let Some(packet) = consumer.pop() {
            taken.push(packet);
        }
        Some(taken)
    }
}

impl OpenDevices for Outputs {
    fn devices(&self) -> impl Iterator<Item = &Device> {
        Outputs::devices(self)
    }

    fn open(&mut self, device: &Device) -> Result<(), OpenError> {
        Outputs::open(self, device)
    }

    fn close(&mut self, id: &DeviceId) -> bool {
        Outputs::close(self, id)
    }

    fn tick(&mut self) -> Vec<Device> {
        Outputs::tick(self)
    }
}

/// The audio thread's side of the app's MIDI out, which hands each block's
/// MIDI to every open output, as the outputs' [`Timing`] says.
///
/// The writer downgrades the plugin's MIDI 2.0 to MIDI 1.0 by the
/// specification's Default Translation, because every device on the systems
/// that Hapto reads now takes only MIDI 1.0. What MIDI 1.0 has no message
/// for is dropped and counted in [`lost`](Self::lost).
///
/// Each output gets the packets in the order the plugin sent them, and its
/// thread sends each when its time comes, or at once if that has passed, so
/// never early and never dropped for being late. Once a block has handed an
/// output its MIDI, the writer wakes the output's thread with a signal that
/// never blocks, so the block's first message leaves the moment the block
/// has played.
///
/// # Examples
///
/// ```
/// use hapto_midi_devices::{Instant, outputs};
/// use hapto_ump::Event;
///
/// let (_outputs, mut writer) = outputs(1024);
/// // Each block, on the audio thread, once the plugin has played it. The
/// // block's first frame is due now, at 48 kHz.
/// let sent = [Event::new(480, [0x40B0_0700, u32::MAX, 0, 0])];
/// writer.write(Instant::now(), 48_000.0, &sent);
/// assert_eq!((writer.dropped(), writer.lost()), (0, 0));
/// ```
pub struct OutputsWriter {
    /// Each output's ring, and the signal that wakes its thread.
    rings: Box<[(Producer, Arc<backend::Signal>)]>,
    timing: Arc<AtomicU8>,
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
    /// Hands `sent`, a block's MIDI, to every open output.
    ///
    /// Each packet is due at once, or, at [`Timing::AtFrames`], at `start`
    /// (the block's first frame) plus its frame at `rate` frames a second. A
    /// packet past an output's room is dropped and counted in
    /// [`dropped`](Self::dropped).
    ///
    /// Thread: audio. Allocates nothing, and wakes each output it handed MIDI
    /// with a signal that never blocks.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn write(&mut self, start: Instant, rate: f64, sent: &[Event]) {
        if sent.is_empty() || !self.rings.iter().any(|(ring, _)| ring.is_read()) {
            return;
        }
        let at_frames = Timing::from_index(self.timing.load(Ordering::Relaxed)) == Timing::AtFrames;
        let nanos_per_frame = if rate > 0.0 { 1e9 / rate } else { 0.0 };
        let downgrade = Downgrade::new();
        let (mut lost, mut pushed) = (0_u64, false);
        let rings = &mut self.rings;
        for event in sent {
            // Due at once, because no moment is earlier than the clock's
            // start.
            let due = if at_frames {
                let after = (f64::from(event.offset) * nanos_per_frame) as u64;
                start.after(Duration::from_nanos(after))
            } else {
                Instant::from_nanos(0)
            };
            let dropped = downgrade.translate(event, |packet| {
                for (ring, _) in rings.iter_mut().filter(|(ring, _)| ring.is_read()) {
                    ring.push(due, packet.words);
                    pushed = true;
                }
            });
            lost = lost.saturating_add(u64::from(dropped));
        }
        if pushed {
            for (_, signal) in rings.iter().filter(|(ring, _)| ring.is_read()) {
                signal.set();
            }
        }
        self.lost = self.lost.saturating_add(lost);
    }

    /// Returns how many packets have been dropped because an open output's
    /// ring was full or they were part of a SysEx longer than its device
    /// sends whole.
    ///
    /// A drop from a full ring means that the device sends too slowly for
    /// what the plugin sends.
    pub fn dropped(&self) -> u64 {
        self.rings.iter().map(|(ring, _)| ring.dropped()).sum()
    }

    /// Returns how many messages have been lost because MIDI 1.0 has no
    /// message for them.
    pub fn lost(&self) -> u64 {
        self.lost
    }
}

#[cfg(test)]
mod tests;
