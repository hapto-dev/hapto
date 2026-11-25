//! The sequencer's list of ports, read client by client.

use std::ffi::{CStr, c_char, c_int};
use std::ptr;

use alsa_sys as sys;

use super::{Address, Caps, Client, Version, ump};

/// One port in the sequencer's list.
#[derive(Debug, Clone)]
pub(crate) struct PortEntry {
    pub(crate) address: Address,
    pub(crate) client_name: String,
    pub(crate) name: String,
    /// The sound card that the client stands for, if any.
    pub(crate) card: Option<u32>,
    pub(crate) caps: Caps,
    /// The client's MIDI version, or MIDI 1.0 if this alsa-lib has no UMP
    /// calls.
    pub(crate) version: Version,
}

impl Client {
    /// Returns every port of every client in the sequencer now, in its
    /// order.
    pub(crate) fn ports(&self) -> Vec<PortEntry> {
        let (Some(mut client), Some(mut port)) = (ClientInfo::new(), PortInfo::new()) else {
            return Vec::new();
        };
        let mut entries = Vec::new();
        while client.next(self) {
            let Some(number) = client.number() else {
                continue;
            };
            let (client_name, card, version) = (client.name(), client.card(), client.version());
            port.start(number);
            while port.next(self) {
                let Some(place) = port.number() else {
                    continue;
                };
                entries.push(PortEntry {
                    address: Address {
                        client: number,
                        port: place,
                    },
                    client_name: client_name.clone(),
                    name: port.name(),
                    card,
                    caps: port.caps(),
                    version,
                });
            }
        }
        entries
    }

    /// Returns whether the port at `at` is in the sequencer now.
    pub(super) fn has_port(&self, at: Address) -> bool {
        // Without memory for the query, the port counts as there.
        let Some(info) = PortInfo::new() else {
            return true;
        };
        // SAFETY: the open handle and the info that the call fills.
        let found = unsafe {
            sys::snd_seq_get_any_port_info(
                self.handle,
                c_int::from(at.client),
                c_int::from(at.port),
                info.0,
            )
        };
        found >= 0
    }
}

/// Returns the string at `text`, or an empty one if it's null.
///
/// # Safety
///
/// `text` is null, or points at a string that alsa-lib keeps through the
/// call.
unsafe fn text(text: *const c_char) -> String {
    if text.is_null() {
        return String::new();
    }
    // SAFETY: as the caller guarantees.
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

/// A client info from alsa-lib, moved through the list client by client and
/// freed when it drops.
struct ClientInfo(*mut sys::snd_seq_client_info_t);

impl ClientInfo {
    /// Creates an info before the list's first client.
    fn new() -> Option<ClientInfo> {
        let mut info = ptr::null_mut();
        // SAFETY: the call writes the pointer.
        if unsafe { sys::snd_seq_client_info_malloc(&raw mut info) } < 0 || info.is_null() {
            return None;
        }
        // SAFETY: the allocated info, and -1 places it before the first client.
        unsafe { sys::snd_seq_client_info_set_client(info, -1) };
        Some(ClientInfo(info))
    }

    /// Moves to the next client of `seq`, returning `false` past the last.
    fn next(&mut self, seq: &Client) -> bool {
        // SAFETY: the handle is open, and the call fills the info.
        unsafe { sys::snd_seq_query_next_client(seq.handle, self.0) >= 0 }
    }

    fn number(&self) -> Option<u8> {
        // SAFETY: the info, filled.
        u8::try_from(unsafe { sys::snd_seq_client_info_get_client(self.0) }).ok()
    }

    fn name(&self) -> String {
        // SAFETY: the info, filled, whose name lives as long as it does.
        unsafe { text(sys::snd_seq_client_info_get_name(self.0)) }
    }

    /// Returns the sound card that the client stands for, if any.
    fn card(&self) -> Option<u32> {
        // SAFETY: the info, filled.
        u32::try_from(unsafe { sys::snd_seq_client_info_get_card(self.0) }).ok()
    }

    fn version(&self) -> Version {
        ump().map_or(Version::Legacy, |ump| {
            // SAFETY: the filled info, and alsa-lib's own function.
            Version::from_raw(unsafe { (ump.info_midi_version)(self.0) })
        })
    }
}

impl Drop for ClientInfo {
    fn drop(&mut self) {
        // SAFETY: the info made, freed once.
        unsafe { sys::snd_seq_client_info_free(self.0) };
    }
}

/// A port info from alsa-lib, moved through a client's ports one by one and
/// freed when it drops.
struct PortInfo(*mut sys::snd_seq_port_info_t);

impl PortInfo {
    fn new() -> Option<PortInfo> {
        let mut info = ptr::null_mut();
        // SAFETY: the call writes the pointer.
        (unsafe { sys::snd_seq_port_info_malloc(&raw mut info) } >= 0 && !info.is_null())
            .then_some(PortInfo(info))
    }

    /// Places the info before the first port of client `client`.
    fn start(&mut self, client: u8) {
        // SAFETY: the allocated info, and -1 places it before the client's
        // first port.
        unsafe {
            sys::snd_seq_port_info_set_client(self.0, c_int::from(client));
            sys::snd_seq_port_info_set_port(self.0, -1);
        }
    }

    /// Moves to the client's next port, returning `false` past the last.
    fn next(&mut self, seq: &Client) -> bool {
        // SAFETY: the handle is open, and the call fills the info.
        unsafe { sys::snd_seq_query_next_port(seq.handle, self.0) >= 0 }
    }

    fn number(&self) -> Option<u8> {
        // SAFETY: the info, filled.
        u8::try_from(unsafe { sys::snd_seq_port_info_get_port(self.0) }).ok()
    }

    fn name(&self) -> String {
        // SAFETY: the info, filled, whose name lives as long as it does.
        unsafe { text(sys::snd_seq_port_info_get_name(self.0)) }
    }

    fn caps(&self) -> Caps {
        // SAFETY: the info, filled.
        Caps(unsafe { sys::snd_seq_port_info_get_capability(self.0) })
    }
}

impl Drop for PortInfo {
    fn drop(&mut self) {
        // SAFETY: the info made, freed once.
        unsafe { sys::snd_seq_port_info_free(self.0) };
    }
}
