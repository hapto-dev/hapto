//! The app's MIDI out: devices opened on the main thread, the audio
//! thread's writer that hands each block's MIDI to every one, and a thread
//! of each device's own that sends it as soon as its block has played, or
//! each message at its frame's time.

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
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use hapto_ump::{Downgrade, Event};

use crate::Instant;
use crate::backend;
use crate::error::OpenError;
use crate::id::{Device, DeviceId, Direction};
use crate::ports::{OpenDevices, Port, Ports};
use crate::ring::{Consumer, Producer, ring};

/// How many outputs may be open at once.
pub const MAX_OUTPUTS: usize = 16;

/// When an output sends a block's MIDI.
///
/// A message from an input is placed in a block by when it came, so it
/// plays a block late, and keeps its timing within one. Sent at once, the
/// block's MIDI reaches a device as soon as the block has played, as JUCE's
/// standalone sends it: the least latency, from no time to a block after
/// its input, but each message a block's length or less early of its frame,
/// so a block's spacing is lost. Sent at its frame's time, each message
/// keeps its place: an arpeggio stays even and a clock steady, and a
/// message from an input goes out a block after it came, every time.
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
    /// As soon as its block has played, as JUCE sends it: the least
    /// latency.
    #[default]
    Immediate,
    /// Each message at its frame's time: the block's spacing kept.
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
/// waiting to go, and splits it into its two sides: [`Outputs`], which opens
/// devices on the main thread, and [`OutputsWriter`], which hands each
/// block's MIDI to them on the audio thread.
///
/// Every output's room is made here, so opening a device later allocates
/// only on the main thread and the device's own.
///
/// # Examples
///
/// ```no_run
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

/// The main thread's side of the app's MIDI out: the devices open now.
///
/// Each device gets every message the plugin sends, whatever its group: a
/// MIDI 1.0 device has none. A device found gone as the output sends to it
/// is closed at the next [`tick`](Self::tick); one that leaves while nothing
/// is sent is closed once the system's list no longer shows it. Every output
/// closes as this drops.
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
    ports: Ports<backend::Output>,
    /// What wakes each output's thread, the writer setting it.
    signals: Box<[Arc<backend::Signal>]>,
    /// When the writer has the outputs send, shared with it.
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
    /// Opens `device`. A device open already stays open.
    ///
    /// # Errors
    ///
    /// Returns why it did not open: an [`OpenError`].
    pub fn open(&mut self, device: &Device) -> Result<(), OpenError> {
        if device.direction() != Direction::Output {
            return Err(OpenError::NotAnOutput);
        }
        let signals = &self.signals;
        self.ports.open(device, |place, mut consumer| {
            let Some(signal) = signals.get(place) else {
                return Err((OpenError::NoRoom, Some(consumer)));
            };
            // What a block pushed before the device came is not its to send.
            consumer.clear();
            backend::Output::open(device, consumer, Arc::clone(signal))
        })
    }

    /// Closes the device `id` names, if it is open, and says whether it was.
    /// What it had not sent yet is dropped with it.
    pub fn close(&mut self, id: &DeviceId) -> bool {
        self.ports.close(id)
    }

    /// Whether the device `id` names is open.
    pub fn is_open(&self, id: &DeviceId) -> bool {
        self.ports.is_open(id)
    }

    /// From the next block on, has every output send as `timing` says.
    pub fn set_timing(&self, timing: Timing) {
        self.timing.store(timing.index(), Ordering::Relaxed);
    }

    /// When the outputs send a block's MIDI.
    pub fn timing(&self) -> Timing {
        Timing::from_index(self.timing.load(Ordering::Relaxed))
    }

    /// The devices open now.
    pub fn devices(&self) -> impl Iterator<Item = &Device> {
        self.ports.devices()
    }

    /// Closes the outputs whose device left while open, and returns them.
    /// Call it once per tick.
    pub fn tick(&mut self) -> Vec<Device> {
        self.ports.tick()
    }

    /// Takes every packet waiting in output `output`'s room, as its device
    /// would, when no device is open there, and marks the room read, so
    /// the writer hands it the next blocks: `None` where a device is open.
    ///
    /// A hook for tests, which have no device to send to.
    #[doc(hidden)]
    pub fn take(&mut self, output: usize) -> Option<Vec<(Instant, [u32; 4])>> {
        let consumer = self.ports.free_mut(output)?;
        consumer.set_reading(true);
        let mut taken = Vec::new();
        while let Some(packet) = consumer.peek() {
            taken.push(packet);
            consumer.pop();
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

/// The audio thread's side of the app's MIDI out: it hands each block's
/// MIDI to every open output, due as soon as the block has played, or each
/// packet at its frame's time, as the outputs' [`Timing`] says.
///
/// The plugin's MIDI 2.0 is downgraded to MIDI 1.0 here, by the
/// specification's Default Translation, since every device on the systems
/// Hapto reads now takes MIDI 1.0 alone: what MIDI 1.0 has no message for
/// is dropped and counted in [`lost`](Self::lost). A packet goes to every
/// open output in the order the plugin sent it, and an output's thread
/// sends it once its time comes, at once if that has passed: never earlier,
/// and never dropped for being late. Once a block has handed an output its
/// MIDI, the writer wakes the output's thread, a signal that never blocks:
/// so the block's first message leaves the moment the block has played.
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
    /// Each output's ring, and what wakes its thread.
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
    /// Hands `sent`, a block's MIDI, to every open output: due at once,
    /// or, at [`Timing::AtFrames`], each packet at `start`, when the
    /// block's first frame is, plus its frame at `rate` frames a second.
    /// Past an output's room a packet is dropped and counted in
    /// [`dropped`](Self::dropped).
    ///
    /// Thread: audio. Allocates nothing, and wakes each output it handed
    /// MIDI with a signal that never blocks.
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
            // Due at once: no moment is earlier than the clock's start.
            let due = if at_frames {
                let after = (f64::from(event.offset) * nanos_per_frame) as u64;
                Instant::from_nanos(start.nanos().saturating_add(after))
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

    /// How many packets found no room in an open output's ring, or were a
    /// SysEx longer than its device sends whole, since it was made: the
    /// device sends too slowly for what the plugin sends.
    pub fn dropped(&self) -> u64 {
        self.rings.iter().map(|(ring, _)| ring.dropped()).sum()
    }

    /// How many messages MIDI 1.0 had no message for, since it was made.
    pub fn lost(&self) -> u64 {
        self.lost
    }
}

#[cfg(test)]
mod tests;
