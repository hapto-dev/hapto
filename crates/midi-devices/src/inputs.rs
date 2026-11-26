//! The app's MIDI in, with devices opened on the main thread and read into
//! each block on the audio thread, merged on one group as a host hands a
//! plugin its MIDI.

// The reader runs on the audio thread, where a panic ends the app's audio,
// so indexing, unwrap, expect, panic and unchecked arithmetic are denied. A
// provably safe site gives its reason in an `#[expect]`.
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

use hapto_ump::{Event, EventList, SysEx7, SysExForm, Upgrade};

use crate::Instant;
use crate::backend;
use crate::error::OpenError;
use crate::id::{Device, DeviceId, Direction};
use crate::ports::{OpenDevices, Port, Ports};
use crate::ring::{Consumer, Producer, ring};

/// How many inputs may be open at once.
pub const MAX_INPUTS: usize = 16;

/// Makes the app's MIDI in, with room in each input for `room` packets
/// between two blocks, and splits it into [`Inputs`] for the main thread and
/// [`InputsReader`] for the audio thread.
///
/// Every input's room and translation state is allocated here, so opening a
/// device later allocates only the system's own buffers, on the main
/// thread. A SysEx that fits in the room usually reaches a block whole, and
/// [`InputsReader`] describes when it doesn't.
///
/// # Examples
///
/// ```no_run
/// use hapto_midi_devices::{Devices, Instant, inputs};
/// use hapto_ump::EventList;
///
/// let (mut inputs, mut reader) = inputs(1024);
/// // On the main thread, every keyboard that the computer has.
/// for device in Devices::new().inputs() {
///     if let Err(why) = inputs.open(device) {
///         eprintln!("{}: {why}", device.name());
///     }
/// }
///
/// // On the audio thread, once per block of 256 frames.
/// let mut midi = EventList::with_capacity(2048);
/// reader.read(Instant::now(), 256, &mut midi);
/// ```
pub fn inputs(room: usize) -> (Inputs, InputsReader) {
    let (producers, consumers): (Vec<_>, Vec<_>) = (0..MAX_INPUTS).map(|_| ring(room)).unzip();
    let inputs = Inputs {
        ports: Ports::new(producers),
    };
    (inputs, InputsReader::new(consumers))
}

/// The main thread's side of the app's MIDI in, with the open devices.
///
/// Every device's messages arrive merged on group 1, as a host hands a
/// plugin its MIDI, so a plugin gets the same MIDI in the app as in a host,
/// and a mapping learned in one works in the other. A device that leaves
/// while open is closed at the next [`tick`](Self::tick), and every input
/// closes when this drops.
///
/// Open: how learn tells devices apart, because a packet doesn't name its
/// device.
///
/// Thread: main.
///
/// # Examples
///
/// ```no_run
/// use hapto_midi_devices::{Devices, inputs};
///
/// let mut devices = Devices::new();
/// let (mut inputs, _reader) = inputs(1024);
/// for device in devices.inputs() {
///     let _ = inputs.open(device);
/// }
///
/// // At each tick, follow the devices that come and go.
/// let changes = devices.tick(std::time::Instant::now());
/// for device in &changes.removed {
///     inputs.close(device.id());
/// }
/// for device in inputs.tick() {
///     println!("{} left", device.name());
/// }
/// for device in inputs.devices() {
///     println!("{} is open", device.name());
/// }
/// ```
pub struct Inputs {
    ports: Ports<backend::Input>,
}

impl Port for backend::Input {
    type Room = Producer;

    fn has_left(&self) -> bool {
        backend::Input::has_left(self)
    }

    fn close(self) -> Option<Producer> {
        backend::Input::close(self)
    }
}

impl fmt::Debug for Inputs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.devices()).finish()
    }
}

impl Inputs {
    /// Opens `device`, unless it is already open.
    ///
    /// # Errors
    ///
    /// Returns an [`OpenError`] if the device did not open.
    pub fn open(&mut self, device: &Device) -> Result<(), OpenError> {
        if device.direction() != Direction::Input {
            return Err(OpenError::NotAnInput);
        }
        self.ports.open(device, |_, mut producer| {
            producer.renew();
            backend::Input::open(device, producer)
        })
    }

    /// Closes the device that `id` names, if it is open, and returns whether
    /// it was.
    pub fn close(&mut self, id: &DeviceId) -> bool {
        self.ports.close(id)
    }

    /// Returns whether the device that `id` names is open.
    pub fn is_open(&self, id: &DeviceId) -> bool {
        self.ports.is_open(id)
    }

    /// Returns the open devices.
    pub fn devices(&self) -> impl Iterator<Item = &Device> {
        self.ports.devices()
    }

    /// Pushes a packet into input `input`'s ring as a device would, if no
    /// device is open there, and returns whether it did.
    ///
    /// A hook for tests, which have no device to read.
    #[doc(hidden)]
    pub fn push(&mut self, input: usize, at: Instant, words: [u32; 4]) -> bool {
        let Some(producer) = self.ports.free_mut(input) else {
            return false;
        };
        producer.push(at, words);
        true
    }

    /// Closes and returns the devices that left while open.
    ///
    /// Call it once per tick.
    pub fn tick(&mut self) -> Vec<Device> {
        self.ports.tick()
    }
}

impl OpenDevices for Inputs {
    fn devices(&self) -> impl Iterator<Item = &Device> {
        Inputs::devices(self)
    }

    fn open(&mut self, device: &Device) -> Result<(), OpenError> {
        Inputs::open(self, device)
    }

    fn close(&mut self, id: &DeviceId) -> bool {
        Inputs::close(self, id)
    }

    fn tick(&mut self) -> Vec<Device> {
        Inputs::tick(self)
    }
}

/// The audio thread's side of the app's MIDI in, which reads every input
/// into each block, merged in the order that the messages came.
///
/// Each message lands at its time's frame, as the time since the last block
/// maps onto this one, so input plays a block late but keeps its timing
/// within the block. A message that came before the last block lands at the
/// first frame.
///
/// Each input is upgraded to MIDI 2.0 by its own translation, as an
/// exporter's MIDI is, so two devices' parameter numbers never mix. A new
/// device on an input starts its translation over.
///
/// A SysEx reaches the block whole once all of it has come, at the frame its
/// end came, and other devices' messages go on meanwhile without cutting
/// into it. One longer than its input's room goes out as it comes.
///
/// # Examples
///
/// ```
/// use hapto_midi_devices::{Instant, inputs};
/// use hapto_ump::EventList;
///
/// let (_inputs, mut reader) = inputs(1024);
/// let mut midi = EventList::with_capacity(2048);
/// // Each block, on the audio thread, before the plugin plays it.
/// midi.clear();
/// reader.read(Instant::now(), 512, &mut midi);
/// assert_eq!(reader.dropped(), 0);
/// ```
pub struct InputsReader {
    lanes: Box<[Lane]>,
    /// When the last block was read.
    last: Option<Instant>,
}

/// One input, as the reader reads it.
struct Lane {
    ring: Consumer,
    /// The input's translation to MIDI 2.0.
    upgrade: Upgrade,
    /// The ring's generation at the last read. A change means a new device
    /// took the ring.
    seen: u32,
    /// The time and packet count of the ring's next message in this block,
    /// found again only after that message is read, so a SysEx held for its
    /// end isn't scanned for every message from another input.
    head: Option<(Instant, usize)>,
}

impl fmt::Debug for InputsReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputsReader")
            .field("last", &self.last)
            .field("dropped", &self.dropped())
            .finish_non_exhaustive()
    }
}

impl InputsReader {
    /// Returns a reader over `consumers`, one per input.
    fn new(consumers: Vec<Consumer>) -> InputsReader {
        let lanes = consumers
            .into_iter()
            .map(|ring| Lane {
                seen: ring.generation(),
                upgrade: Upgrade::new(),
                head: None,
                ring,
            })
            .collect();
        InputsReader { lanes, last: None }
    }

    /// Reads every message that came before `now` into `out`, at frames of
    /// a block of `frames` frames.
    ///
    /// - Messages are in the order they came across every input, with ties
    ///   in input order.
    /// - A block of no frames reads nothing, so its messages wait for the
    ///   next.
    /// - A message past `out`'s room is dropped and counted in `out`.
    ///
    /// Thread: audio. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn read(&mut self, now: Instant, frames: usize, out: &mut EventList) {
        if frames == 0 {
            return;
        }
        let last = self.last.replace(now).unwrap_or(now).min(now);
        for lane in &mut self.lanes {
            lane.head = ready(&lane.ring, now);
        }
        // The earliest message across the inputs, or on a tie, the first
        // input's.
        while let Some((at, count, lane)) = self
            .lanes
            .iter_mut()
            .filter_map(|lane| lane.head.map(|(at, count)| (at, count, lane)))
            .min_by_key(|(at, _, _)| *at)
        {
            // The generation is read after the ring's head, so a new device's
            // packet comes with it and the translation starts over first.
            let generation = lane.ring.generation();
            if generation != lane.seen {
                lane.upgrade.reset();
                lane.seen = generation;
            }
            let frame = frame(at, last, now, frames);
            for _ in 0..count {
                if let Some((_, words)) = lane.ring.pop() {
                    lane.upgrade.event(&Event::new(frame, words), out);
                }
            }
            lane.head = ready(&lane.ring, now);
        }
    }

    /// Returns how many messages have been dropped because their input's ring
    /// was full.
    ///
    /// A drop means that the audio thread read too seldom for how fast the
    /// messages came.
    pub fn dropped(&self) -> u64 {
        self.lanes.iter().map(|lane| lane.ring.dropped()).sum()
    }
}

/// The most packets that a device hands over at once, a SysEx buffer of
/// 1024 bytes at six bytes a packet, plus one for what the stream held.
pub(crate) const BURST: usize = 172;

/// Returns a SysEx7 packet's form, or `None` for any other packet.
fn sysex_form(words: [u32; 4]) -> Option<SysExForm> {
    SysEx7::decode(words).map(|packet| packet.form)
}

/// Returns the time and packet count of the next message that `ring` has
/// ready for a block that starts at `now`.
///
/// A SysEx is ready once its end has come, as every packet to its end, the
/// real-time messages among them included, at its end's time. Until then it
/// waits, unless the ring has less room left than [`BURST`] and is at least
/// half full. It then goes out a packet at a time as it came, so the
/// device's next buffer of it still fits.
fn ready(ring: &Consumer, now: Instant) -> Option<(Instant, usize)> {
    let (at, words) = ring.peek()?;
    if at >= now {
        return None;
    }
    if sysex_form(words) != Some(SysExForm::Start) {
        return Some((at, 1));
    }
    let mut latest = at;
    for index in 1..ring.len() {
        let (when, words) = ring.peek_at(index)?;
        if when >= now {
            return None;
        }
        match sysex_form(words) {
            Some(SysExForm::End) => return Some((when, index.saturating_add(1))),
            Some(SysExForm::Continue) => latest = when,
            _ if Event::new(0, words).is_real_time() => {}
            // The device cut the SysEx short with another message, so what
            // came goes out as it is.
            _ => return Some((latest, index)),
        }
    }
    let full = ring
        .capacity()
        .saturating_sub(BURST)
        .max(ring.capacity().div_euclid(2));
    (ring.len() >= full).then_some((at, 1))
}

/// Returns the frame of `at` when the time from `last` to `now` maps onto a
/// block of `frames` frames, at least one.
fn frame(at: Instant, last: Instant, now: Instant, frames: usize) -> u32 {
    let span = u128::from(now.nanos().saturating_sub(last.nanos()));
    let into = u128::from(at.nanos().saturating_sub(last.nanos()));
    // An empty span puts everything at the first frame.
    let frame = into
        .saturating_mul(frames as u128)
        .checked_div(span)
        .unwrap_or(0);
    u32::try_from(frame)
        .unwrap_or(u32::MAX)
        .min(Event::last_offset(frames))
}

#[cfg(test)]
mod tests;
