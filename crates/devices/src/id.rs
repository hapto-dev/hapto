//! What names a device, and what the list says of it.

use std::fmt;

/// Whether a device sends MIDI to the computer or takes it.
///
/// # Examples
///
/// ```
/// use hapto_devices::{Devices, Direction};
///
/// let devices = Devices::new();
/// assert!(devices.inputs().all(|device| device.direction() == Direction::Input));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Direction {
    /// It sends MIDI: a keyboard, a controller.
    Input,
    /// It takes MIDI: a synth, a port out.
    Output,
}

impl Direction {
    #[cfg_attr(
        all(not(windows), not(test)),
        expect(
            dead_code,
            reason = "only a backend names a device, and this system has none"
        )
    )]
    fn tag(self) -> &'static str {
        match self {
            Direction::Input => "in",
            Direction::Output => "out",
        }
    }
}

/// What names a device from one look at the system's list to the next.
///
/// It survives the system's renumbering, which moves a device's place in
/// the list when another comes or goes. On WinMM it is the device's
/// interface path, or its name for one that has none, with its place among
/// the devices that share that string and its direction. Compare it, keep
/// it and show it in a log; read nothing into its text, which only says
/// which device it names.
///
/// # Examples
///
/// ```
/// use hapto_devices::Devices;
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
    /// The id of the `place`th device, from 0, whose backend names it by
    /// `key` in `direction`.
    #[cfg_attr(
        all(not(windows), not(test)),
        expect(
            dead_code,
            reason = "only a backend names a device, and this system has none"
        )
    )]
    pub(crate) fn new(direction: Direction, key: &str, place: usize) -> DeviceId {
        DeviceId(format!("{}:{key}#{place}", direction.tag()))
    }

    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A MIDI device the system lists: its id, its name as the system shows
/// it, and its direction.
///
/// # Examples
///
/// ```
/// use hapto_devices::Devices;
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
    #[cfg_attr(
        all(not(windows), not(test)),
        expect(
            dead_code,
            reason = "only a backend names a device, and this system has none"
        )
    )]
    pub(crate) fn new(id: DeviceId, name: String, direction: Direction) -> Device {
        Device {
            id,
            name,
            direction,
        }
    }

    /// What names it from one look at the list to the next.
    pub fn id(&self) -> &DeviceId {
        &self.id
    }

    /// Its name, as the system shows it. On WinMM the system cuts it at 31
    /// characters.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether it sends MIDI or takes it.
    pub fn direction(&self) -> Direction {
        self.direction
    }
}

/// Names each of `named`, in the system's order, by its key and its place
/// among the ones before it with the same key.
#[cfg_attr(
    all(not(windows), not(test)),
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
