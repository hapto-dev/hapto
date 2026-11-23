//! The app's MIDI in: devices opened on the main thread, each on a group of
//! its own, and read into each block on the audio thread.

use std::error::Error;
use std::fmt;

use hapto_ump::{Event, EventList};

use crate::Instant;
use crate::backend;
use crate::id::{Device, DeviceId, Direction};
use crate::ring::{Consumer, Producer, ring};

/// How many inputs may be open at once: one for each of UMP's groups.
pub const GROUPS: usize = 16;

/// Makes the app's MIDI in, with room in each input for `room` packets
/// between two blocks, and splits it into its two sides: [`Inputs`], which
/// opens devices on the main thread, and [`InputsReader`], which reads them
/// into each block on the audio thread.
///
/// Every input's room is made here, so opening a device later allocates
/// only the system's own buffers, on the main thread.
///
/// # Examples
///
/// ```
/// use hapto_devices::{Devices, Instant, inputs};
/// use hapto_ump::EventList;
///
/// let (mut inputs, mut reader) = inputs(1024);
/// // On the main thread: every keyboard the computer has, each on a group.
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
    let (producers, consumers): (Vec<_>, Vec<_>) = (0..GROUPS).map(|_| ring(room)).unzip();
    let inputs = Inputs {
        slots: producers.into_iter().map(Slot::Free).collect(),
    };
    let reader = InputsReader {
        rings: consumers.into_boxed_slice(),
        last: None,
    };
    (inputs, reader)
}

/// The main thread's side of the app's MIDI in: the devices open now, each
/// on the group it was given.
///
/// [`open`](Self::open) gives a device the lowest group free, and the
/// device's messages arrive on that group, so devices never mix. A device
/// that leaves while open is closed at the next [`tick`](Self::tick). Every
/// input closes as this drops.
///
/// Thread: main.
///
/// # Examples
///
/// ```
/// use hapto_devices::{Devices, inputs};
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
/// for (device, group) in inputs.devices() {
///     println!("{} on group {}", device.name(), group + 1);
/// }
/// ```
pub struct Inputs {
    slots: Vec<Slot>,
}

enum Slot {
    /// No device: the room waits for one.
    Free(Producer),
    /// A device, reading into the room.
    Open(Device, backend::Input),
    /// A device the system would not close, whose room went with it.
    Lost,
}

impl fmt::Debug for Inputs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.devices()).finish()
    }
}

/// Why a device did not open.
///
/// # Examples
///
/// ```
/// use hapto_devices::{Devices, OpenError, inputs};
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
    /// The device takes MIDI and sends none.
    NotAnInput,
    /// The device is not in the system's list now.
    Gone,
    /// Another app holds it: WinMM lets one app at a time read a device.
    InUse,
    /// Every group has a device on it already.
    NoRoom,
    /// The system had no memory for it.
    NoMemory,
    /// No backend is built for this system.
    NoBackend,
    /// The system refused it, with this code of its own.
    System(u32),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenError::NotAnInput => f.write_str("the device takes MIDI and sends none"),
            OpenError::Gone => f.write_str("the device is not there now"),
            OpenError::InUse => f.write_str("another app holds the device"),
            OpenError::NoRoom => write!(f, "{GROUPS} devices are open already, one per group"),
            OpenError::NoMemory => f.write_str("the system had no memory for the device"),
            OpenError::NoBackend => f.write_str("Hapto reads no MIDI devices on this system yet"),
            OpenError::System(code) => write!(f, "the system refused the device (error {code})"),
        }
    }
}

impl Error for OpenError {}

impl Inputs {
    /// Opens `device` on the lowest group free, and returns the group, 0
    /// to 15. A device open already keeps its group.
    ///
    /// # Errors
    ///
    /// Returns why it did not open: an [`OpenError`].
    pub fn open(&mut self, device: &Device) -> Result<u8, OpenError> {
        if device.direction() != Direction::Input {
            return Err(OpenError::NotAnInput);
        }
        if let Some(group) = self.group(device.id()) {
            return Ok(group);
        }
        let (index, slot) = self
            .slots
            .iter_mut()
            .enumerate()
            .find(|(_, slot)| matches!(slot, Slot::Free(_)))
            .ok_or(OpenError::NoRoom)?;
        let group = u8::try_from(index).unwrap_or(u8::MAX);
        let Slot::Free(producer) = std::mem::replace(slot, Slot::Lost) else {
            return Err(OpenError::NoRoom);
        };
        match backend::Input::open(device, group, producer) {
            Ok(input) => {
                *slot = Slot::Open(device.clone(), input);
                Ok(group)
            }
            Err((why, producer)) => {
                *slot = producer.map_or(Slot::Lost, Slot::Free);
                Err(why)
            }
        }
    }

    /// Closes the device `id` names, if it is open, and says whether it was.
    pub fn close(&mut self, id: &DeviceId) -> bool {
        let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| matches!(slot, Slot::Open(device, _) if device.id() == id))
        else {
            return false;
        };
        if let Slot::Open(_, input) = std::mem::replace(slot, Slot::Lost) {
            *slot = input.close().map_or(Slot::Lost, Slot::Free);
        }
        true
    }

    /// The group the device `id` names is open on.
    pub fn group(&self, id: &DeviceId) -> Option<u8> {
        self.devices()
            .find(|(device, _)| device.id() == id)
            .map(|(_, group)| group)
    }

    /// The devices open now, each with its group.
    pub fn devices(&self) -> impl Iterator<Item = (&Device, u8)> {
        self.slots
            .iter()
            .zip(0..)
            .filter_map(|(slot, group)| match slot {
                Slot::Open(device, _) => Some((device, group)),
                Slot::Free(_) | Slot::Lost => None,
            })
    }

    /// Pushes a packet into `group`'s ring as a device would, when no
    /// device is open on it, and says whether it did.
    ///
    /// A hook for tests, which have no device to read.
    #[doc(hidden)]
    pub fn push(&mut self, group: u8, at: Instant, words: [u32; 4]) -> bool {
        match self.slots.get_mut(usize::from(group)) {
            Some(Slot::Free(producer)) => {
                producer.push(at, words);
                true
            }
            _ => false,
        }
    }

    /// Closes the inputs whose device left while open, and returns them.
    /// Call it once per tick.
    pub fn tick(&mut self) -> Vec<Device> {
        let left: Vec<Device> = self
            .slots
            .iter()
            .filter_map(|slot| match slot {
                Slot::Open(device, input) if input.has_left() => Some(device.clone()),
                _ => None,
            })
            .collect();
        for device in &left {
            self.close(device.id());
        }
        left
    }
}

/// The audio thread's side of the app's MIDI in: it reads every input into
/// each block, in the order the messages came.
///
/// A message lands at the frame its time takes in the block, as the time
/// since the last block maps onto this one: so input plays a block late,
/// and keeps its own timing within one. A message that came before the last
/// block lands at the first frame. Each is MIDI 1.0 as the device sent it,
/// on its input's group, for the engine to upgrade.
///
/// # Examples
///
/// ```
/// use hapto_devices::{Instant, inputs};
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
    rings: Box<[Consumer]>,
    /// When the last block was read.
    last: Option<Instant>,
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
    /// Reads every message that came before `now` into `out`, at frames of
    /// a block of `frames` frames, in the order they came across every
    /// input; messages that came at the same moment keep their groups'
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
        loop {
            let mut earliest: Option<(usize, Instant, [u32; 4])> = None;
            for (index, ring) in self.rings.iter().enumerate() {
                if let Some((at, words)) = ring.peek()
                    && at < now
                    && earliest.is_none_or(|(_, first, _)| at < first)
                {
                    earliest = Some((index, at, words));
                }
            }
            let Some((index, at, words)) = earliest else {
                break;
            };
            if let Some(ring) = self.rings.get_mut(index) {
                ring.pop();
            }
            out.push(Event::new(frame(at, last, now, frames), words));
        }
    }

    /// How many messages found no room in their input's ring, since it was
    /// made: the audio thread read too seldom for how fast they came.
    pub fn dropped(&self) -> u64 {
        self.rings.iter().map(Consumer::dropped).sum()
    }
}

/// The frame of a block of `frames` frames, at least one, that `at` takes,
/// as the time from `last` to `now` maps onto the block.
fn frame(at: Instant, last: Instant, now: Instant, frames: usize) -> u32 {
    let span = u128::from(now.nanos().saturating_sub(last.nanos()));
    let into = u128::from(at.nanos().saturating_sub(last.nanos()));
    let last_frame = frames.saturating_sub(1);
    // A span of nothing puts everything at the first frame.
    let frame = into
        .saturating_mul(frames as u128)
        .checked_div(span)
        .map_or(0, |frame| usize::try_from(frame).unwrap_or(last_frame));
    u32::try_from(frame.min(last_frame)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
