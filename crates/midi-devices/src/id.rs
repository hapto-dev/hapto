//! A device's id, and what the list says about it.

use std::fmt;

/// Whether a device sends MIDI to the computer or takes it.
///
/// # Examples
///
/// ```
/// use hapto_midi_devices::{Devices, Direction};
///
/// let devices = Devices::new();
/// assert!(devices.inputs().all(|device| device.direction() == Direction::Input));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Direction {
    /// A device that sends MIDI, such as a keyboard or a controller.
    Input,
    /// A device that takes MIDI, such as a synth or an output port.
    Output,
}

impl Direction {
    fn tag(self) -> &'static str {
        match self {
            Direction::Input => "in",
            Direction::Output => "out",
        }
    }
}

/// What identifies a device from one look at the system's list to the
/// next.
///
/// It survives the system's renumbering, which moves a device's place in
/// the list when another comes or goes. It is a string that identifies the
/// device, with the device's place among the devices that share that
/// string, and its direction. On WinMM the string is the device's interface
/// path, or its name for a device without one. On ALSA it is, for a card's
/// port, the card's USB vendor, product and serial number, or where a card
/// without one sits on its bus, then the port's number; for a software
/// port, its client's name and the port's own. Compare it, keep it and show
/// it in a log; read nothing into its text, which only says which device it
/// identifies.
///
/// # Examples
///
/// ```
/// use hapto_midi_devices::Devices;
///
/// let devices = Devices::new();
/// for device in devices.all() {
///     // The same device, found again by its id.
///     assert_eq!(devices.get(device.id()), Some(device));
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeviceId(String);

impl DeviceId {
    /// Returns the id of the device at `place`, from 0, among those in
    /// `direction` that the backend names by `key`.
    pub(crate) fn new(direction: Direction, key: &str, place: usize) -> DeviceId {
        DeviceId(format!("{}:{key}#{place}", direction.tag()))
    }

    /// Returns the id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A MIDI device in the system's list, with its id, name and direction.
///
/// # Examples
///
/// ```
/// use hapto_midi_devices::Devices;
///
/// for device in Devices::new().all() {
///     println!("{:?} {}: {}", device.direction(), device.name(), device.id());
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Device {
    id: DeviceId,
    name: String,
    direction: Direction,
}

impl Device {
    pub(crate) fn new(id: DeviceId, name: String, direction: Direction) -> Device {
        Device {
            id,
            name,
            direction,
        }
    }

    /// Returns a device named `name` that no system lists, for other crates'
    /// tests.
    #[doc(hidden)]
    pub fn for_tests(name: &str, direction: Direction) -> Device {
        Device::new(
            DeviceId::new(direction, name, 0),
            name.to_owned(),
            direction,
        )
    }

    /// Returns the device's id.
    pub fn id(&self) -> &DeviceId {
        &self.id
    }

    /// Returns the device's name as the system shows it, which WinMM cuts at
    /// 31 characters.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns whether the device sends or takes MIDI.
    pub fn direction(&self) -> Direction {
        self.direction
    }
}

/// Returns a device for each `(name, key)` in `named`, in order, with an id
/// from its key and its place among the earlier ones with that key.
#[cfg_attr(
    all(not(hapto_backend), not(test)),
    expect(
        dead_code,
        reason = "only a backend names a device, and this system has none"
    )
)]
pub(crate) fn named(direction: Direction, named: Vec<(String, String)>) -> Vec<Device> {
    let mut seen: Vec<&str> = Vec::new();
    let mut devices = Vec::with_capacity(named.len());
    for (name, key) in &named {
        let place = seen.iter().filter(|&&earlier| earlier == key).count();
        seen.push(key);
        devices.push(Device::new(
            DeviceId::new(direction, key, place),
            name.clone(),
            direction,
        ));
    }
    devices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices_that_share_a_key_are_told_apart_by_their_place() {
        let entries = vec![
            ("Keys".to_owned(), "path-a".to_owned()),
            ("Keys".to_owned(), "path-b".to_owned()),
            ("Pads".to_owned(), "path-a".to_owned()),
        ];
        let ids: Vec<String> = named(Direction::Input, entries)
            .iter()
            .map(|device| device.id().to_string())
            .collect();
        assert_eq!(ids, ["in:path-a#0", "in:path-b#0", "in:path-a#1"]);
        let out = named(
            Direction::Output,
            vec![("Synth".to_owned(), "Synth".to_owned())],
        );
        assert_eq!(out[0].id().as_str(), "out:Synth#0");
    }
}
