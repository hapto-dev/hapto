//! A sequencer client of the app's own, wrapped so the rest of the backend
//! calls alsa-lib safely: the client here, the events it reads and sends in
//! `event`, the list of ports in `list`, and the UMP calls in `ump`.

mod event;
mod list;
mod ump;

use std::ffi::{CStr, CString, c_int, c_uint};
use std::ops::BitOr;
use std::ptr;
use std::sync::OnceLock;

use alsa_sys as sys;

pub(super) use event::{Incoming, Parser, SendError};
pub(super) use list::PortEntry;

use crate::error::OpenError;
use crate::id::Direction;
use ump::{Ump, ump};

// The header's macros, which the bindings leave out.
const OPEN_OUTPUT: c_int = 1;
const OPEN_INPUT: c_int = 2;
const NONBLOCK: c_int = 1;
const TYPE_MIDI_GENERIC: c_uint = 1 << 1;
const TYPE_APPLICATION: c_uint = 1 << 20;

/// The System client, which every kernel has.
pub(super) const SYSTEM: u8 = 0;
/// The System client's port that announces every client and port that
/// comes, goes or changes.
pub(super) const ANNOUNCE: Address = Address {
    client: SYSTEM,
    port: 1,
};

/// The most events the kernel lets a client hold waiting to be read. Each
/// holds one event, or 28 bytes of a SysEx.
const ROOM: usize = 2000;

/// A port's place in the sequencer: its client's number and its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Address {
    pub(super) client: u8,
    pub(super) port: u8,
}

impl From<sys::snd_seq_addr> for Address {
    fn from(address: sys::snd_seq_addr) -> Address {
        Address {
            client: address.client,
            port: address.port,
        }
    }
}

impl From<Address> for sys::snd_seq_addr {
    fn from(address: Address) -> sys::snd_seq_addr {
        sys::snd_seq_addr {
            client: address.client,
            port: address.port,
        }
    }
}

/// What a port lets other apps do with it: the sequencer's capability
/// bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Caps(c_uint);

impl Caps {
    pub(super) const READ: Caps = Caps(1 << 0);
    pub(super) const WRITE: Caps = Caps(1 << 1);
    pub(super) const SUBS_READ: Caps = Caps(1 << 5);
    pub(super) const SUBS_WRITE: Caps = Caps(1 << 6);
    pub(super) const NO_EXPORT: Caps = Caps(1 << 7);
    const INACTIVE: Caps = Caps(1 << 8);
    const UMP_ENDPOINT: Caps = Caps(1 << 9);

    /// Whether another app may subscribe to read from the port, for an
    /// input, or to write to it, for an output.
    pub(super) fn allows(self, direction: Direction) -> bool {
        self.has(match direction {
            Direction::Input => Caps::READ | Caps::SUBS_READ,
            Direction::Output => Caps::WRITE | Caps::SUBS_WRITE,
        })
    }

    /// Whether the port keeps out of other apps' lists, or is not in use.
    pub(super) fn is_hidden(self) -> bool {
        self.0 & (Caps::NO_EXPORT | Caps::INACTIVE).0 != 0
    }

    /// Whether the port stands for a MIDI 2.0 device's whole endpoint.
    pub(super) fn is_endpoint(self) -> bool {
        self.has(Caps::UMP_ENDPOINT)
    }

    fn has(self, caps: Caps) -> bool {
        self.0 & caps.0 == caps.0
    }
}

impl BitOr for Caps {
    type Output = Caps;

    fn bitor(self, other: Caps) -> Caps {
        Caps(self.0 | other.0)
    }
}

/// The MIDI a client reads and writes: MIDI 1.0's events, or UMP at either
/// protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Version {
    Legacy,
    UmpMidi1,
    UmpMidi2,
}

impl Version {
    fn from_raw(raw: c_int) -> Version {
        match raw {
            1 => Version::UmpMidi1,
            2 => Version::UmpMidi2,
            _ => Version::Legacy,
        }
    }

    fn raw(self) -> c_int {
        match self {
            Version::Legacy => 0,
            Version::UmpMidi1 => 1,
            Version::UmpMidi2 => 2,
        }
    }
}

/// Which way a client's MIDI goes: `Input` only reads, and `Duplex`, an
/// output's, writes too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Streams {
    Input,
    Duplex,
}

/// Whether this alsa-lib has the UMP calls, so the app's clients can speak
/// MIDI 2.0.
pub(super) fn has_ump() -> bool {
    ump().is_some()
}

/// Why a call refused, from the negative error number alsa-lib returns.
fn refused(code: c_int) -> OpenError {
    match code.saturating_neg() {
        libc::ENOMEM => OpenError::NoMemory,
        libc::EBUSY => OpenError::InUse,
        libc::ENOENT | libc::ENXIO | libc::ENODEV => OpenError::Gone,
        other => OpenError::System(u32::try_from(other).unwrap_or(0)),
    }
}

/// `code`, if alsa-lib did not refuse.
fn check(code: c_int) -> Result<c_int, OpenError> {
    if code < 0 {
        Err(refused(code))
    } else {
        Ok(code)
    }
}

/// `text` as a C string, any NUL in it left out.
fn c_string(text: &str) -> CString {
    CString::new(text.replace('\0', "")).unwrap_or_default()
}

/// What the app's clients are named: the app's own name.
fn app_name() -> &'static CStr {
    static NAME: OnceLock<CString> = OnceLock::new();
    NAME.get_or_init(|| {
        let name = std::env::current_exe()
            .ok()
            .and_then(|path| Some(path.file_stem()?.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "Hapto".to_owned());
        c_string(&name)
    })
}

/// A client of the sequencer, of the app's own. It closes as it drops.
#[derive(Debug)]
pub(super) struct Client {
    handle: *mut sys::snd_seq_t,
    /// The UMP calls, for a client that speaks UMP; `None` for a MIDI 1.0
    /// one.
    ump: Option<&'static Ump>,
    /// The device the client is for, if it is one's: see
    /// [`for_device`](Self::for_device).
    device: Option<Address>,
}

// SAFETY: alsa-lib's handle may move between threads; each client is used
// by one thread at a time, the one that holds it, or under a lock.
unsafe impl Send for Client {}

impl Client {
    /// Opens a non-blocking client named after the app, for `streams`, at
    /// MIDI `version`. A UMP version is refused as no backend where this
    /// alsa-lib has no UMP calls.
    pub(super) fn open(streams: Streams, version: Version) -> Result<Client, OpenError> {
        let streams = match streams {
            Streams::Input => OPEN_INPUT,
            Streams::Duplex => OPEN_INPUT | OPEN_OUTPUT,
        };
        let mut handle = ptr::null_mut();
        // SAFETY: the handle is written by the call; "default" names the
        // sequencer in alsa-lib's configuration.
        check(unsafe {
            sys::snd_seq_open(&raw mut handle, c"default".as_ptr(), streams, NONBLOCK)
        })?;
        if handle.is_null() {
            return Err(OpenError::NoMemory);
        }
        let mut client = Client {
            handle,
            ump: None,
            device: None,
        };
        // SAFETY: the open handle and a name that lives as long as the app.
        unsafe { sys::snd_seq_set_client_name(client.handle, app_name().as_ptr()) };
        if version != Version::Legacy {
            let ump = ump().ok_or(OpenError::NoBackend)?;
            // SAFETY: the open handle; alsa-lib's own function, found by
            // its name.
            check(unsafe { (ump.set_midi_version)(client.handle, version.raw()) })?;
            client.ump = Some(ump);
        }
        Ok(client)
    }

    /// Raises the client's room for waiting events from the kernel's 200 to
    /// [`ROOM`], so a SysEx dump that comes at once waits while the
    /// client's thread wakes, rather than being dropped. alsa-lib's own read
    /// buffer is made as long first, so a SysEx another app sends as one
    /// event, which the room may hold whole, is never too long to read.
    pub(super) fn with_input_room(self) -> Client {
        let bytes = ROOM.saturating_mul(size_of::<sys::snd_seq_ump_event_t>());
        // SAFETY: the open handle; a buffer alsa-lib cannot make leaves the
        // one it has.
        if unsafe { sys::snd_seq_set_input_buffer_size(self.handle, bytes) } >= 0 {
            // SAFETY: the open handle; a size the kernel refuses changes
            // nothing.
            unsafe { sys::snd_seq_set_client_pool_input(self.handle, ROOM) };
        }
        self
    }

    /// Makes the client the one for the device at `device`. It then reads
    /// only what that device sends and what the System client announces,
    /// passing over what another app sends straight to its port. After an
    /// overrun, which may have dropped the announcement that the device
    /// left, it asks whether the device's port is still there, and reads
    /// that it left if not.
    pub(super) fn for_device(mut self, device: Address) -> Client {
        self.device = Some(device);
        self
    }

    /// Makes a port of the client's own, `name`, with `caps`, and returns
    /// its number.
    pub(super) fn port(&self, name: &str, caps: Caps) -> Result<u8, OpenError> {
        let name = c_string(name);
        // SAFETY: the open handle and a name that lives through the call.
        let port = check(unsafe {
            sys::snd_seq_create_simple_port(
                self.handle,
                name.as_ptr(),
                caps.0,
                TYPE_MIDI_GENERIC | TYPE_APPLICATION,
            )
        })?;
        u8::try_from(port).map_err(|_| OpenError::NoRoom)
    }

    /// Subscribes the client's `port` to what `from` sends.
    pub(super) fn connect_from(&self, port: u8, from: Address) -> Result<(), OpenError> {
        // SAFETY: the open handle; the call only subscribes.
        check(unsafe {
            sys::snd_seq_connect_from(
                self.handle,
                c_int::from(port),
                c_int::from(from.client),
                c_int::from(from.port),
            )
        })
        .map(drop)
    }

    /// Subscribes `to` to what the client's `port` sends.
    pub(super) fn connect_to(&self, port: u8, to: Address) -> Result<(), OpenError> {
        // SAFETY: the open handle; the call only subscribes.
        check(unsafe {
            sys::snd_seq_connect_to(
                self.handle,
                c_int::from(port),
                c_int::from(to.client),
                c_int::from(to.port),
            )
        })
        .map(drop)
    }

    /// The descriptors a poll waits on for an event to come in.
    pub(super) fn poll_fds(&self) -> Vec<libc::pollfd> {
        // SAFETY: the open handle.
        let count = unsafe { sys::snd_seq_poll_descriptors_count(self.handle, libc::POLLIN) };
        let count = c_uint::try_from(count).unwrap_or(0);
        let empty = libc::pollfd {
            fd: -1,
            events: 0,
            revents: 0,
        };
        let mut fds = vec![empty; count as usize];
        // SAFETY: room for `count` descriptors, which the call fills.
        let filled = unsafe {
            sys::snd_seq_poll_descriptors(self.handle, fds.as_mut_ptr().cast(), count, libc::POLLIN)
        };
        fds.truncate(usize::try_from(filled).unwrap_or(0));
        fds
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: the open handle, closed once; its ports and subscriptions
        // go with it.
        unsafe { sys::snd_seq_close(self.handle) };
    }
}
