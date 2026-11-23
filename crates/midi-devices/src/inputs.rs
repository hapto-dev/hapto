//! The app's MIDI in: devices opened on the main thread, and read into each
//! block on the audio thread, merged on one group as a host hands a plugin
//! its MIDI.

// Audio-path code: the reader runs on the audio thread, where a panic ends
// the app's audio, so indexing, unwrap, expect, panic and unchecked
// arithmetic are refused, and a site that is provably safe says why in an
// `#[expect]`.
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
/// between two blocks, and splits it into its two sides: [`Inputs`], which
/// opens devices on the main thread, and [`InputsReader`], which reads them
/// into each block on the audio thread.
///
/// Every input's room and translation state is made here, so opening a
/// device later allocates only the system's own buffers, on the main
/// thread. Room enough for a SysEx lets it reach a block whole: see
/// [`InputsReader`].
///
/// # Examples
///
/// ```no_run
/// use hapto_midi_devices::{Devices, Instant, inputs};
/// use hapto_ump::EventList;
///
/// let (mut inputs, mut reader) = inputs(1024);
/// // On the main thread: every keyboard the computer has.
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

/// The main thread's side of the app's MIDI in: the devices open now.
///
/// Every device's messages arrive on group 1, merged, as a host hands a
/// plugin its MIDI, so a plugin hears in the app what it hears in a host,
/// and a mapping learned in one plays in the other. A device that leaves
/// while open is closed at the next [`tick`](Self::tick). Every input closes
/// as this drops.
///
/// Open: learn telling two devices apart, which a packet does not say.
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
/// // At each tick: follow the devices that come and go.
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
    /// Opens `device`. A device open already stays open.
    ///
    /// # Errors
    ///
    /// Returns why it did not open: an [`OpenError`].
    pub fn open(&mut self, device: &Device) -> Result<(), OpenError> {
        if device.direction() != Direction::Input {
            return Err(OpenError::NotAnInput);
        }
        self.ports.open(device, |_, mut producer| {
            producer.renew();
            backend::Input::open(device, producer)
        })
    }

    /// Closes the device `id` names, if it is open, and says whether it was.
    pub fn close(&mut self, id: &DeviceId) -> bool {
        self.ports.close(id)
    }

    /// Whether the device `id` names is open.
    pub fn is_open(&self, id: &DeviceId) -> bool {
        self.ports.is_open(id)
    }

    /// The devices open now.
    pub fn devices(&self) -> impl Iterator<Item = &Device> {
        self.ports.devices()
    }

    /// Pushes a packet into input `input`'s ring as a device would, when no
    /// device is open there, and says whether it did.
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

    /// Closes the inputs whose device left while open, and returns them.
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

/// The audio thread's side of the app's MIDI in: it reads every input into
/// each block, merged in the order the messages came.
///
/// A message lands at the frame its time takes in the block, as the time
/// since the last block maps onto this one: so input plays a block late,
/// and keeps its own timing within one. A message that came before the last
/// block lands at the first frame.
///
/// Each input is upgraded to MIDI 2.0 with a translation of its own, as an
/// exporter's MIDI is, so two devices' parameter numbers never mix, and a
/// new device on an input starts it over. A SysEx reaches the block whole,
/// at the frame its end came, once all of it has: another device's message
/// never cuts it, and goes on meanwhile. One longer than its input's room
/// goes out as it comes instead.
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
    /// The generation of the ring the reader last read: a change means a
    /// new device took it.
    seen: u32,
    /// The ring's next message, when it came and how many packets it is,
    /// for the block being read: found once, and again only once it was
    /// read, so a SysEx held for its end is not looked over for every
    /// message another input sends.
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
    /// A reader over `consumers`, an input each.
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
    /// a block of `frames` frames, in the order they came across every
    /// input; messages that came at the same moment keep their inputs'
    /// order. A block of no frames reads nothing, so its messages wait for
    /// the next. Past `out`'s room a message is dropped and counted there.
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
        // The earliest message across the inputs, the first input's of
        // those that came at the same moment.
        while let Some((at, count, lane)) = self
            .lanes
            .iter_mut()
            .filter_map(|lane| lane.head.map(|(at, count)| (at, count, lane)))
            .min_by_key(|(at, _, _)| *at)
        {
            // Read after the ring's head, so a packet a new device pushed
            // comes with the new device's generation, and its translation
            // starts over before it.
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
        for lane in &mut self.lanes {
            lane.upgrade.end_block(Event::last_offset(frames), out);
        }
    }

    /// How many messages found no room in their input's ring, since it was
    /// made: the audio thread read too seldom for how fast they came.
    pub fn dropped(&self) -> u64 {
        self.lanes.iter().map(|lane| lane.ring.dropped()).sum()
    }
}

/// The most packets a device hands over at once: a SysEx buffer of 1024
/// bytes, six bytes a packet, and one more for what the stream held.
pub(crate) const BURST: usize = 172;

/// A SysEx7 packet's form, or `None` for any other packet.
fn sysex_form(words: [u32; 4]) -> Option<SysExForm> {
    SysEx7::decode(words).map(|packet| packet.form)
}

/// What `ring` has ready for a block that starts at `now`: when its next
/// message came, and how many packets that message is.
///
/// A SysEx is ready once its end has come, and is every packet to its end,
/// the real-time messages among them included, at the time its end came.
/// Until then it waits, unless the ring has less room left than a device
/// hands over at once, [`BURST`], and is at least half full, when it goes
/// out a packet at a time as it came: so a device's next buffer of it still
/// fits.
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
            // A SysEx the device dropped part way, then something else:
            // what came of it goes as it is.
            _ => return Some((latest, index)),
        }
    }
    let full = ring
        .capacity()
        .saturating_sub(BURST)
        .max(ring.capacity().div_euclid(2));
    (ring.len() >= full).then_some((at, 1))
}

/// The frame of a block of `frames` frames, at least one, that `at` takes,
/// as the time from `last` to `now` maps onto the block.
fn frame(at: Instant, last: Instant, now: Instant, frames: usize) -> u32 {
    let span = u128::from(now.nanos().saturating_sub(last.nanos()));
    let into = u128::from(at.nanos().saturating_sub(last.nanos()));
    // A span of nothing puts everything at the first frame.
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
