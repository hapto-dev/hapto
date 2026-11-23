//! The slots inputs and outputs keep their devices in, and what the two
//! share.

use crate::error::OpenError;
use crate::id::{Device, DeviceId};

/// A backend's open device, an input or an output, over the room it reads
/// into or sends from.
pub(crate) trait Port: Sized {
    /// The room: a ring's producer for an input, its consumer for an
    /// output.
    type Room;

    /// Whether the device left while open.
    fn has_left(&self) -> bool;

    /// Closes it, handing its room back; `None` when the system would not
    /// close it, and the room went with it.
    fn close(self) -> Option<Self::Room>;
}

/// One room, and what holds it.
enum Slot<P: Port> {
    /// No device: the room waits for one.
    Free(P::Room),
    /// A device, open on the room.
    Open(Device, P),
    /// A device the system would not close, or whose thread ended, whose
    /// room went with it.
    Lost,
}

impl<P: Port> Slot<P> {
    /// Its room, when it is free, leaving it held until the device opens.
    fn take_free(&mut self) -> Option<P::Room> {
        match std::mem::replace(self, Slot::Lost) {
            Slot::Free(room) => Some(room),
            other => {
                *self = other;
                None
            }
        }
    }

    /// Closes its device, if one is open, and takes back the room.
    fn shut(&mut self) {
        *self = match std::mem::replace(self, Slot::Lost) {
            Slot::Open(_, port) => port.close().map_or(Slot::Lost, Slot::Free),
            other => other,
        };
    }

    fn device(&self) -> Option<&Device> {
        match self {
            Slot::Open(device, _) => Some(device),
            Slot::Free(_) | Slot::Lost => None,
        }
    }
}

/// A fixed set of rooms for one direction's devices.
pub(crate) struct Ports<P: Port> {
    slots: Vec<Slot<P>>,
}

impl<P: Port> Ports<P> {
    pub(crate) fn new(rooms: impl IntoIterator<Item = P::Room>) -> Self {
        Ports {
            slots: rooms.into_iter().map(Slot::Free).collect(),
        }
    }

    /// Opens `device` in the first free room, by `open`, which takes the
    /// room's place and the room and hands back the port, or why not, with
    /// the room if it is whole. A device open already stays open.
    pub(crate) fn open(
        &mut self,
        device: &Device,
        open: impl FnOnce(usize, P::Room) -> Result<P, (OpenError, Option<P::Room>)>,
    ) -> Result<(), OpenError> {
        if self.is_open(device.id()) {
            return Ok(());
        }
        let (place, slot, room) = self
            .slots
            .iter_mut()
            .enumerate()
            .find_map(|(place, slot)| {
                Some((place, slot.take_free()?)).map(|(place, room)| (place, slot, room))
            })
            .ok_or(OpenError::NoRoom)?;
        match open(place, room) {
            Ok(port) => {
                *slot = Slot::Open(device.clone(), port);
                Ok(())
            }
            Err((why, room)) => {
                *slot = room.map_or(Slot::Lost, Slot::Free);
                Err(why)
            }
        }
    }

    /// Closes the device `id` names, if it is open, and says whether it was.
    pub(crate) fn close(&mut self, id: &DeviceId) -> bool {
        let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| slot.device().is_some_and(|device| device.id() == id))
        else {
            return false;
        };
        slot.shut();
        true
    }

    /// The devices open now.
    pub(crate) fn devices(&self) -> impl Iterator<Item = &Device> {
        self.slots.iter().filter_map(Slot::device)
    }

    /// Whether the device `id` names is open.
    pub(crate) fn is_open(&self, id: &DeviceId) -> bool {
        self.devices().any(|device| device.id() == id)
    }

    /// Closes the devices that left while open, and returns them.
    pub(crate) fn tick(&mut self) -> Vec<Device> {
        let mut left = Vec::new();
        for slot in &mut self.slots {
            if let Slot::Open(device, port) = slot
                && port.has_left()
            {
                left.push(device.clone());
                slot.shut();
            }
        }
        left
    }

    /// The room at `place`, when no device is open on it.
    pub(crate) fn free_mut(&mut self, place: usize) -> Option<&mut P::Room> {
        match self.slots.get_mut(place) {
            Some(Slot::Free(room)) => Some(room),
            _ => None,
        }
    }
}

/// What [`Inputs`](crate::Inputs) and [`Outputs`](crate::Outputs) share:
/// the devices one direction has open, and opening and closing them, for
/// code that follows both alike.
///
/// # Examples
///
/// ```no_run
/// use hapto_midi_devices::{Device, OpenDevices, inputs, outputs};
///
/// // Opens every listed device of one direction.
/// fn open_all<'a>(ends: &mut impl OpenDevices, listed: impl Iterator<Item = &'a Device>) {
///     for device in listed {
///         let _ = ends.open(device);
///     }
/// }
///
/// let devices = hapto_midi_devices::Devices::new();
/// let (mut ins, _reader) = inputs(1024);
/// let (mut outs, _writer) = outputs(1024);
/// open_all(&mut ins, devices.inputs());
/// open_all(&mut outs, devices.outputs());
/// ```
pub trait OpenDevices {
    /// The devices open now.
    fn devices(&self) -> impl Iterator<Item = &Device>;

    /// Opens `device`. A device open already stays open.
    ///
    /// # Errors
    ///
    /// Returns why it did not open: an [`OpenError`].
    fn open(&mut self, device: &Device) -> Result<(), OpenError>;

    /// Closes the device `id` names, if it is open, and says whether it was.
    fn close(&mut self, id: &DeviceId) -> bool;

    /// Closes the devices that left while open, and returns them. Call it
    /// once per tick.
    fn tick(&mut self) -> Vec<Device>;
}
