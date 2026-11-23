//! The app's MIDI in: devices opened on the main thread, and read into each
//! block on the audio thread, merged on one group as a host hands a plugin
//! its MIDI.

use std::error::Error;
use std::fmt;

use hapto_ump::{Event, EventList, Upgrade};

use crate::Instant;
use crate::backend;
use crate::id::{Device, DeviceId, Direction};
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
/// ```
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
        slots: producers.into_iter().map(Slot::Free).collect(),
    };
    let reader = InputsReader {
        upgrades: (0..MAX_INPUTS).map(|_| Upgrade::new()).collect(),
        generations: consumers.iter().map(Consumer::generation).collect(),
        rings: consumers.into_boxed_slice(),
        last: None,
    };
    (inputs, reader)
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
/// ```
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
        f.debug_list().entries(self.devices()).finish()
    }
}

/// Why a device did not open.
///
/// # Examples
///
/// ```
/// use hapto_midi_devices::{Devices, OpenError, inputs};
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
    /// The device takes MIDI and sends none, so it cannot be read.
    NotAnInput,
    /// The device sends MIDI and takes none, so nothing can be sent to it.
    NotAnOutput,
    /// The device is not in the system's list now.
    Gone,
    /// Another app holds it: WinMM lets one app at a time open a device.
    InUse,
    /// [`MAX_INPUTS`] inputs, or [`MAX_OUTPUTS`](crate::MAX_OUTPUTS)
    /// outputs, are open already.
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
            OpenError::NotAnOutput => f.write_str("the device sends MIDI and takes none"),
            OpenError::Gone => f.write_str("the device is not there now"),
            OpenError::InUse => f.write_str("another app holds the device"),
            OpenError::NoRoom => write!(f, "{MAX_INPUTS} devices are open already"),
            OpenError::NoMemory => f.write_str("the system had no memory for the device"),
            OpenError::NoBackend => f.write_str("Hapto reads no MIDI devices on this system yet"),
            OpenError::System(code) => write!(f, "the system refused the device (error {code})"),
        }
    }
}

impl Error for OpenError {}

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
        if self.is_open(device.id()) {
            return Ok(());
        }
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| matches!(slot, Slot::Free(_)))
            .ok_or(OpenError::NoRoom)?;
        let Slot::Free(mut producer) = std::mem::replace(slot, Slot::Lost) else {
            return Err(OpenError::NoRoom);
        };
        producer.renew();
        match backend::Input::open(device, producer) {
            Ok(input) => {
                *slot = Slot::Open(device.clone(), input);
                Ok(())
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

    /// Pushes a packet into input `input`'s ring as a device would, when no
    /// device is open there, and says whether it did.
    ///
    /// A hook for tests, which have no device to read.
    #[doc(hidden)]
    pub fn push(&mut self, input: usize, at: Instant, words: [u32; 4]) -> bool {
        match self.slots.get_mut(input) {
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
    rings: Box<[Consumer]>,
    /// Each input's translation to MIDI 2.0.
    upgrades: Box<[Upgrade]>,
    /// The generation of each ring the reader last read: a change means a
    /// new device took it.
    generations: Box<[u32]>,
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
        self.start_over_where_renewed();
        loop {
            let mut earliest: Option<(usize, Instant, usize)> = None;
            for (index, ring) in self.rings.iter().enumerate() {
                if let Some((at, count)) = ready(ring, now)
                    && earliest.is_none_or(|(_, first, _)| at < first)
                {
                    earliest = Some((index, at, count));
                }
            }
            let Some((index, at, count)) = earliest else {
                break;
            };
            let (Some(ring), Some(upgrade)) =
                (self.rings.get_mut(index), self.upgrades.get_mut(index))
            else {
                break;
            };
            let frame = frame(at, last, now, frames);
            for _ in 0..count {
                if let Some((_, words)) = ring.peek() {
                    upgrade.event(&Event::new(frame, words), out);
                }
                ring.pop();
            }
        }
        let last_frame = u32::try_from(frames.saturating_sub(1)).unwrap_or(u32::MAX);
        for upgrade in &mut self.upgrades {
            upgrade.end_block(last_frame, out);
        }
    }

    /// Forgets each input's translation state where a new device took the
    /// input since the last block.
    fn start_over_where_renewed(&mut self) {
        let inputs = self.rings.iter().zip(&mut self.upgrades);
        for ((ring, upgrade), seen) in inputs.zip(&mut self.generations) {
            let generation = ring.generation();
            if generation != *seen {
                upgrade.reset();
                *seen = generation;
            }
        }
    }

    /// How many messages found no room in their input's ring, since it was
    /// made: the audio thread read too seldom for how fast they came.
    pub fn dropped(&self) -> u64 {
        self.rings.iter().map(Consumer::dropped).sum()
    }
}

/// The SysEx7 statuses a run of packets turns on.
const SYSEX_START: u32 = 0x1;
const SYSEX_CONTINUE: u32 = 0x2;
const SYSEX_END: u32 = 0x3;

/// A SysEx7 packet's status, or `None` for any other packet.
fn sysex_status(words: [u32; 4]) -> Option<u32> {
    (words[0] >> 28 == 0x3).then_some((words[0] >> 20) & 0xF)
}

/// Whether `words` is a real-time system message, which may fall inside a
/// SysEx.
fn real_time(words: [u32; 4]) -> bool {
    words[0] >> 28 == 0x1 && (words[0] >> 16) & 0xFF >= 0xF8
}

/// What `ring` has ready for a block that starts at `now`: when its next
/// message came, and how many packets that message is.
///
/// A SysEx is ready once its end has come, and is every packet to its end,
/// the real-time messages among them included, at the time its end came.
/// Until then it waits, unless it fills the ring, when it goes out a packet
/// at a time as it came.
fn ready(ring: &Consumer, now: Instant) -> Option<(Instant, usize)> {
    let (at, words) = ring.peek()?;
    if at >= now {
        return None;
    }
    if sysex_status(words) != Some(SYSEX_START) {
        return Some((at, 1));
    }
    let mut latest = at;
    for index in 1..ring.len() {
        let (when, words) = ring.peek_at(index)?;
        if when >= now {
            return None;
        }
        match sysex_status(words) {
            Some(SYSEX_END) => return Some((when, index + 1)),
            Some(SYSEX_CONTINUE) => latest = when,
            _ if real_time(words) => {}
            // A SysEx the device dropped part way, then something else:
            // what came of it goes as it is.
            _ => return Some((latest, index)),
        }
    }
    (ring.len() >= ring.capacity()).then_some((at, 1))
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
