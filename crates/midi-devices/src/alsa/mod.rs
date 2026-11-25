//! The ALSA backend: Linux's MIDI devices, through the sequencer, which
//! shares each device with every app that subscribes to it. Its calls into
//! alsa-lib are in `seq` and `signal`, each with why it holds.
//!
//! Each open device is a sequencer client of the app's own, of the
//! device's own MIDI version, with one port subscribed to the device's and
//! a thread that waits on it: so the kernel translates nothing between the
//! two, and each device owns what it opened, as a WinMM one does. Every
//! port of the app's is hidden from other apps, so no list holds one.

mod input;
mod output;
mod seq;
mod signal;
#[cfg(test)]
mod tests;
mod watch;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

pub(crate) use input::Input;
pub(crate) use output::Output;
use seq::{
    Address, CAP_INACTIVE, CAP_NO_EXPORT, CAP_READ, CAP_SUBS_READ, CAP_SUBS_WRITE,
    CAP_UMP_ENDPOINT, CAP_WRITE, Client, PortEntry, Streams, Version,
};
pub(crate) use signal::Signal;
pub(crate) use watch::Watch;

use crate::id::{Device, DeviceId, Direction, named};

/// A port the list holds: where it is, what names it, and its client's
/// MIDI version.
#[derive(Debug, Clone)]
pub(crate) struct Listed {
    address: Address,
    name: String,
    key: String,
    version: Version,
    caps: u32,
}

impl Listed {
    /// Whether another app may read from it, for an input, or write to it,
    /// for an output.
    fn allows(&self, direction: Direction) -> bool {
        let needs = match direction {
            Direction::Input => CAP_READ | CAP_SUBS_READ,
            Direction::Output => CAP_WRITE | CAP_SUBS_WRITE,
        };
        self.caps & needs == needs
    }
}

/// Every device, inputs then outputs, each in the sequencer's order.
pub(crate) fn list() -> Vec<Device> {
    let listed = listed();
    let mut devices: Vec<Device> = devices_of(&listed, Direction::Input)
        .into_iter()
        .map(|(device, _)| device)
        .collect();
    devices.extend(
        devices_of(&listed, Direction::Output)
            .into_iter()
            .map(|(device, _)| device),
    );
    devices
}

/// The port `id` names in `direction` now. The list is read again, since
/// a client's number moves when it comes back.
pub(crate) fn find(direction: Direction, id: &DeviceId) -> Option<Listed> {
    let listed = listed();
    devices_of(&listed, direction)
        .into_iter()
        .find(|(device, _)| device.id() == id)
        .map(|(_, port)| port.clone())
}

/// The devices of `direction`, each with the port it is.
fn devices_of(listed: &[Listed], direction: Direction) -> Vec<(Device, &Listed)> {
    let ports: Vec<&Listed> = listed
        .iter()
        .filter(|port| port.allows(direction))
        .collect();
    let entries = ports
        .iter()
        .map(|port| (port.name.clone(), port.key.clone()))
        .collect();
    named(direction, entries).into_iter().zip(ports).collect()
}

/// Every port the list holds, in the sequencer's order: read by a client
/// the app keeps for it, since a client that came and went for each look
/// would mark the list changed every time. One that could not open is
/// opened again at the next look: the sequencer may come later, once its
/// module loads or the user's session may open it.
fn listed() -> Vec<Listed> {
    static LISTER: Mutex<Option<Client>> = Mutex::new(None);
    let entries = {
        let mut lister = LISTER.lock().unwrap_or_else(PoisonError::into_inner);
        if lister.is_none() {
            *lister = Client::open(Streams::Input, Version::Legacy).ok();
        }
        match lister.as_ref() {
            Some(client) => client.ports(),
            None => return Vec::new(),
        }
    };
    let ump = seq::ump().is_some();
    let mut cards = HashMap::new();
    entries
        .iter()
        .filter(|entry| is_listed(entry, &entries, ump))
        .map(|entry| Listed {
            address: entry.address,
            name: name(entry),
            key: key(entry, &mut cards),
            version: if ump { entry.version } else { Version::Legacy },
            caps: entry.caps,
        })
        .collect()
}

/// Whether `entry`, one of `entries`, is a device: a port another app may
/// subscribe to, but the System client's. A MIDI 2.0 device's client has a
/// port for its whole endpoint and one for each group: the endpoint's is
/// listed where the app's clients can speak UMP, and the groups' where
/// they cannot.
fn is_listed(entry: &PortEntry, entries: &[PortEntry], ump: bool) -> bool {
    if entry.address.client == seq::SYSTEM || entry.caps & (CAP_NO_EXPORT | CAP_INACTIVE) != 0 {
        return false;
    }
    let endpoint = |port: &PortEntry| port.caps & CAP_UMP_ENDPOINT != 0;
    let has_endpoint = entries
        .iter()
        .any(|port| port.address.client == entry.address.client && endpoint(port));
    !has_endpoint || endpoint(entry) == ump
}

/// What shows of a port: its own name, or its client's for a MIDI 2.0
/// device's endpoint, whose port is named for its protocol.
fn name(entry: &PortEntry) -> String {
    if entry.caps & CAP_UMP_ENDPOINT != 0 {
        entry.client_name.clone()
    } else {
        entry.name.clone()
    }
}

/// What names a port from one look at the list to the next. A card's port
/// is named by the card and the port's number, the card read from sysfs,
/// so it survives the card's renumbering: a USB card whose device has a
/// serial number by that device, so it keeps its name in any socket, and
/// any other card by where it sits on its bus. A software port is named by
/// its client's name and its own. `cards` keeps each card's name, read once
/// for the look.
fn key(entry: &PortEntry, cards: &mut HashMap<u32, String>) -> String {
    match entry.card {
        Some(card) => {
            let place = cards.entry(card).or_insert_with(|| {
                std::fs::canonicalize(format!("/sys/class/sound/card{card}/device")).map_or_else(
                    |_| format!("card{card}"),
                    |path| usb_key(&path).unwrap_or_else(|| path.display().to_string()),
                )
            });
            format!("{place}:{}", entry.address.port)
        }
        None => format!("{}:{}", entry.client_name, entry.name),
    }
}

/// What names the card on the USB interface at `interface` in sysfs, if
/// the interface's device has a serial number: the device's vendor, product
/// and serial, and which of its interfaces the card is on.
fn usb_key(interface: &Path) -> Option<String> {
    // An interface's folder, in its device's, is named
    // `<bus>-<ports>:<configuration>.<interface>`.
    let (_, which) = interface.file_name()?.to_str()?.split_once(':')?;
    let device = interface.parent()?;
    let read = |name: &str| {
        let text = std::fs::read_to_string(device.join(name)).ok()?;
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_owned())
    };
    Some(format!(
        "usb:{}:{}:{}:{which}",
        read("idVendor")?,
        read("idProduct")?,
        read("serial")?
    ))
}
