//! The slots that inputs and outputs keep their devices in, and what the two
//! share.

use crate::error::OpenError;
use crate::id::{Device, DeviceId};

/// A backend's open input or output, with the room it reads into or sends
/// from.
pub(crate) trait Port: Sized {
    /// A ring's producer for an input, or its consumer for an output.
    type Room;

    /// Returns whether the device left while open.
    fn has_left(&self) -> bool;

    /// Closes the device and returns its room, or `None` if the system would
    /// not close it and the room was lost.
    fn close(self) -> Option<Self::Room>;
}

/// One room, and what holds it.
enum Slot<P: Port> {
    /// No device, so the room is free.
    Free(P::Room),
    /// A device open on the room.
    Open(Device, P),
    /// A device that the system would not close, or whose thread ended. Its
    /// room went with it.
    Lost,
}

impl<P: Port> Slot<P> {
    /// Takes the room if the slot is free, leaving the slot `Lost` until the
    /// caller sets it.
    fn take_free(&mut self) -> Option<P::Room> {
        match std::mem::replace(self, Slot::Lost) {
            Slot::Free(room) => Some(room),
            other => {
                *self = other;
                None
            }
        }
    }

    /// Closes the slot's device, if one is open, and takes back the room.
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

    /// Opens `device` in the first free room with `open`, unless it is
    /// already open.
    ///
    /// `open` takes the room's place and the room, and returns the port, or
    /// the error and the room unless the room was lost.
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

    /// Closes the device that `id` names, if it is open, and returns whether
    /// it was.
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

    /// Returns the open devices.
    pub(crate) fn devices(&self) -> impl Iterator<Item = &Device> {
        self.slots.iter().filter_map(Slot::device)
    }

    /// Returns whether the device that `id` names is open.
    pub(crate) fn is_open(&self, id: &DeviceId) -> bool {
        self.devices().any(|device| device.id() == id)
    }

    /// Closes and returns the devices that left while open.
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

    /// Returns the room at `place` if it is free.
    pub(crate) fn free_mut(&mut self, place: usize) -> Option<&mut P::Room> {
        match self.slots.get_mut(place) {
            Some(Slot::Free(room)) => Some(room),
            _ => None,
        }
    }
}

/// What [`Inputs`](crate::Inputs) and [`Outputs`](crate::Outputs) share, for
/// code that handles both alike.
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
    /// Returns the open devices.
    fn devices(&self) -> impl Iterator<Item = &Device>;

    /// Opens `device`, unless it is already open.
    ///
    /// # Errors
    ///
    /// Returns an [`OpenError`] if the device did not open.
    fn open(&mut self, device: &Device) -> Result<(), OpenError>;

    /// Closes the device that `id` names, if it is open, and returns whether
    /// it was.
    fn close(&mut self, id: &DeviceId) -> bool;

    /// Closes and returns the devices that left while open.
    ///
    /// Call it once per tick.
    fn tick(&mut self) -> Vec<Device>;
}
