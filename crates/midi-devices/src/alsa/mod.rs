//! The ALSA backend: Linux's MIDI devices, through the sequencer, which
//! shares each device with every app that subscribes to it. Its unsafe
//! calls are in `seq`, into alsa-lib, and in `signal` and `worker`, into
//! libc's eventfd and ppoll, each with why it holds.
//!
//! Each open device has a sequencer client of the app's own, with one port
//! connected to the device's and a thread that waits on it, so each device
//! owns what it opened, as a WinMM one does. An input's client speaks the
//! device's own MIDI version, so the kernel translates nothing; an output's
//! speaks MIDI 1.0. No other app may subscribe to a port of the app's, so
//! no list holds one.

mod input;
mod output;
mod seq;
mod signal;
#[cfg(test)]
mod tests;
mod watch;
mod worker;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

pub(crate) use input::Input;
pub(crate) use output::Output;
use seq::{Address, Caps, Client, PortEntry, Streams, Version};
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
    caps: Caps,
}

/// Every device, inputs then outputs, each in the sequencer's order.
pub(crate) fn list() -> Vec<Device> {
    let listed = listed();
    [Direction::Input, Direction::Output]
        .into_iter()
        .flat_map(|direction| devices_of(&listed, direction))
        .map(|(device, _)| device)
        .collect()
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
        .filter(|port| port.caps.allows(direction))
        .collect();
    let entries = ports
        .iter()
        .map(|port| (port.name.clone(), port.key.clone()))
        .collect();
    named(direction, entries).into_iter().zip(ports).collect()
}

/// Returns every port the list holds, in the sequencer's order. A client
/// the app keeps reads them, since a client opened and closed for each
/// look would itself mark the list changed every time. If that client
/// cannot open, the next look tries again: the sequencer may come later,
/// once its module loads or the user's session may open it.
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
    let ump = seq::has_ump();
    let mut cards = HashMap::new();
    entries
        .iter()
        .filter(|entry| is_listed(entry, &entries, ump))
        .map(|entry| Listed {
            address: entry.address,
            name: name(entry),
            key: key(entry, &mut cards),
            version: entry.version,
            caps: entry.caps,
        })
        .collect()
}

/// Whether `entry`, one of `entries`, is a device: a port another app may
/// subscribe to, other than the System client's. A MIDI 2.0 device's client
/// has a port for its whole endpoint and one for each group. Where the
/// app's clients can speak UMP, `ump`, the endpoint's port is listed, so
/// the device opens once with all its groups; where they cannot, the
/// groups' ports are.
fn is_listed(entry: &PortEntry, entries: &[PortEntry], ump: bool) -> bool {
    if entry.address.client == seq::SYSTEM || entry.caps.is_hidden() {
        return false;
    }
    let has_endpoint = entries
        .iter()
        .any(|port| port.address.client == entry.address.client && port.caps.is_endpoint());
    !has_endpoint || entry.caps.is_endpoint() == ump
}

/// The name a port shows: its own, or its client's for a MIDI 2.0
/// device's endpoint, whose port is named for its protocol.
fn name(entry: &PortEntry) -> String {
    if entry.caps.is_endpoint() {
        entry.client_name.clone()
    } else {
        entry.name.clone()
    }
}

/// Returns the key that identifies a port from one look at the list to the
/// next. A card's port is keyed by its card and the port's number; a
/// software port by its client's name and its own. `cards` keeps each
/// card's part of the key for the look.
fn key(entry: &PortEntry, cards: &mut HashMap<u32, String>) -> String {
    match entry.card {
        Some(card) => {
            let place = cards.entry(card).or_insert_with(|| card_key(card));
            format!("{place}:{}", entry.address.port)
        }
        None => format!("{}:{}", entry.client_name, entry.name),
    }
}

/// Returns sound card `card`'s part of its ports' keys, found in sysfs so
/// it survives the card's renumbering. A USB card whose device has a
/// serial number is keyed by that device, so a keyboard keeps its key in
/// any socket; any other card by where it sits on its bus.
fn card_key(card: u32) -> String {
    match std::fs::canonicalize(format!("/sys/class/sound/card{card}/device")) {
        Ok(path) => usb_key(&path).unwrap_or_else(|| path.display().to_string()),
        // A card sysfs cannot place keeps its number, which renumbering
        // may move.
        Err(_) => format!("card{card}"),
    }
}

/// Returns the key of the card on the USB interface at `interface` in
/// sysfs, if the interface's device has a serial number: the device's
/// vendor, product and serial, and which of its interfaces the card is on.
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
