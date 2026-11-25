//! A sequencer client of the app's own, and what alsa-lib's sequencer
//! hands out, wrapped so the rest of the backend calls it safely.

use std::ffi::{CStr, CString, c_char, c_int, c_long, c_uint, c_void};
use std::ptr;
use std::sync::OnceLock;

use alsa_sys as sys;

use crate::error::OpenError;

// The header's macros, which the bindings leave out.
const OPEN_OUTPUT: c_int = 1;
const OPEN_INPUT: c_int = 2;
const NONBLOCK: c_int = 1;
pub(super) const CAP_READ: c_uint = 1 << 0;
pub(super) const CAP_WRITE: c_uint = 1 << 1;
pub(super) const CAP_SUBS_READ: c_uint = 1 << 5;
pub(super) const CAP_SUBS_WRITE: c_uint = 1 << 6;
pub(super) const CAP_NO_EXPORT: c_uint = 1 << 7;
pub(super) const CAP_INACTIVE: c_uint = 1 << 8;
pub(super) const CAP_UMP_ENDPOINT: c_uint = 1 << 9;
const TYPE_MIDI_GENERIC: c_uint = 1 << 1;
const TYPE_APPLICATION: c_uint = 1 << 20;
const QUEUE_DIRECT: u8 = 253;
const ADDRESS_SUBSCRIBERS: u8 = 254;
const ADDRESS_UNKNOWN: u8 = 253;
const LENGTH_MASK: u8 = 3 << 2;
const LENGTH_VARIABLE: u8 = 1 << 2;
const EVENT_UMP: u8 = 1 << 5;
const EVENT_SYSEX: u8 = 130;
const EVENT_NONE: u8 = 255;
const EVENT_CLIENT_START: u8 = 60;
const EVENT_CLIENT_EXIT: u8 = 61;
const EVENT_CLIENT_CHANGE: u8 = 62;
const EVENT_PORT_START: u8 = 63;
const EVENT_PORT_EXIT: u8 = 64;
const EVENT_PORT_CHANGE: u8 = 65;

/// The System client, which every kernel has.
pub(super) const SYSTEM: u8 = 0;
/// The System client's port that announces every client and port that
/// comes, goes or changes.
pub(super) const ANNOUNCE: Address = Address {
    client: SYSTEM,
    port: 1,
};

/// A port's place in the sequencer: its client's number and its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Address {
    pub(super) client: u8,
    pub(super) port: u8,
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

/// Which way a client's MIDI goes: an input only reads, an output also
/// writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Streams {
    Input,
    Duplex,
}

// The UMP calls' types, as seq.h declares them.
type SetMidiVersion = unsafe extern "C" fn(*mut sys::snd_seq_t, c_int) -> c_int;
type InfoMidiVersion = unsafe extern "C" fn(*const sys::snd_seq_client_info_t) -> c_int;
type EventInput =
    unsafe extern "C" fn(*mut sys::snd_seq_t, *mut *mut sys::snd_seq_ump_event_t) -> c_int;
type EventOutput =
    unsafe extern "C" fn(*mut sys::snd_seq_t, *mut sys::snd_seq_ump_event_t) -> c_int;

/// alsa-lib's UMP calls, looked up when first needed rather than linked:
/// an alsa-lib older than 1.2.10 has none, and Rust links a Linux binary to
/// resolve every function it calls as it loads.
pub(super) struct Ump {
    set_midi_version: SetMidiVersion,
    info_midi_version: InfoMidiVersion,
    event_input: EventInput,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "only a test's device sends UMP")
    )]
    event_output_direct: EventOutput,
}

/// The UMP calls, if this alsa-lib has them.
pub(super) fn ump() -> Option<&'static Ump> {
    static UMP: OnceLock<Option<Ump>> = OnceLock::new();
    UMP.get_or_init(|| {
        // SAFETY: each name is alsa-lib's, and each found function is taken
        // as the type seq.h declares for it.
        unsafe {
            Some(Ump {
                set_midi_version: std::mem::transmute::<*mut c_void, SetMidiVersion>(symbol(
                    c"snd_seq_set_client_midi_version",
                )?),
                info_midi_version: std::mem::transmute::<*mut c_void, InfoMidiVersion>(symbol(
                    c"snd_seq_client_info_get_midi_version",
                )?),
                event_input: std::mem::transmute::<*mut c_void, EventInput>(symbol(
                    c"snd_seq_ump_event_input",
                )?),
                event_output_direct: std::mem::transmute::<*mut c_void, EventOutput>(symbol(
                    c"snd_seq_ump_event_output_direct",
                )?),
            })
        }
    })
    .as_ref()
}

/// The function `name` in the libraries loaded already, alsa-lib among
/// them.
fn symbol(name: &CStr) -> Option<*mut c_void> {
    // SAFETY: a lookup by a name that lives through the call; a name no
    // library has comes back null.
    let found = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
    (!found.is_null()).then_some(found)
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

/// The text alsa-lib's `text` points at, or nothing.
///
/// # Safety
///
/// `text` is null, or points at a string alsa-lib keeps through the call.
unsafe fn text(text: *const c_char) -> String {
    if text.is_null() {
        return String::new();
    }
    // SAFETY: as the caller says.
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

/// What the app's clients are named: the app's own name.
fn app_name() -> CString {
    let name = std::env::current_exe()
        .ok()
        .and_then(|path| Some(path.file_stem()?.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "Hapto".to_owned());
    CString::new(name.replace('\0', "")).unwrap_or_default()
}

/// What one port of the sequencer's list is.
#[derive(Debug, Clone)]
pub(super) struct PortEntry {
    pub(super) address: Address,
    pub(super) client_name: String,
    pub(super) name: String,
    /// The sound card the client stands for, if it is a card's.
    pub(super) card: Option<u32>,
    pub(super) caps: c_uint,
    pub(super) version: Version,
}

/// One event read from a client.
pub(super) enum Incoming<'a> {
    /// A UMP packet's words, from a device that speaks UMP.
    Packet([u32; 4]),
    /// MIDI 1.0's bytes: a whole message, or a piece of a SysEx.
    Bytes(&'a [u8]),
    /// The System client tells of a client or port.
    Announced(Change),
    /// The client's room ran out: the kernel dropped what came meanwhile.
    Overrun,
    /// Anything else.
    Other,
}

/// What a read answers once the client's room ran out.
const OVERRUN: c_int = -libc::ENOSPC;

/// What the System client told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Change {
    /// A client came or changed, or a port did: the list may differ.
    Listed,
    /// A client left, with every port it had.
    ClientLeft(u8),
    /// A port left.
    PortLeft(Address),
    /// A subscription came or went: the list is the same.
    Other,
}

impl Change {
    /// Whether the port at `port` is gone.
    pub(super) fn ends(self, port: Address) -> bool {
        match self {
            Change::ClientLeft(client) => client == port.client,
            Change::PortLeft(left) => left == port,
            Change::Listed | Change::Other => false,
        }
    }

    /// Whether the list may differ.
    pub(super) fn lists(self) -> bool {
        !matches!(self, Change::Other)
    }
}

/// How many events a client with room may hold waiting to be read: the
/// kernel's most. Each is an event, or 28 bytes of a SysEx.
const ROOM: usize = 2000;

/// A client of the sequencer, of the app's own. It closes as it drops.
#[derive(Debug)]
pub(super) struct Client {
    handle: *mut sys::snd_seq_t,
    version: Version,
    /// The one port whose events it reads, with the System client's
    /// announcements, if it was told one.
    from: Option<Address>,
}

// SAFETY: alsa-lib's handle may move between threads; each client is used
// by one thread at a time, the one that holds it, or under a lock.
unsafe impl Send for Client {}

impl Client {
    /// Opens a client named after the app, non-blocking, for `streams`, at
    /// MIDI `version`, which must be the legacy one where [`ump`] is
    /// `None`.
    pub(super) fn open(streams: Streams, version: Version) -> Result<Client, OpenError> {
        let streams = match streams {
            Streams::Input => OPEN_INPUT,
            Streams::Duplex => OPEN_INPUT | OPEN_OUTPUT,
        };
        let mut handle = ptr::null_mut();
        // SAFETY: the handle is written by the call; "default" names the
        // sequencer in alsa-lib's configuration.
        let opened =
            unsafe { sys::snd_seq_open(&raw mut handle, c"default".as_ptr(), streams, NONBLOCK) };
        // Refused for no memory also where every one of the 64 numbers the
        // kernel gives apps' clients, across the system, is taken.
        if opened < 0 {
            return Err(refused(opened));
        }
        if handle.is_null() {
            return Err(OpenError::NoMemory);
        }
        let mut client = Client {
            handle,
            version: Version::Legacy,
            from: None,
        };
        let name = app_name();
        // SAFETY: the open handle and a name that lives through the call.
        unsafe { sys::snd_seq_set_client_name(client.handle, name.as_ptr()) };
        if version != Version::Legacy {
            let ump = ump().ok_or(OpenError::NoBackend)?;
            // SAFETY: the open handle; alsa-lib's own function, found by
            // its name.
            check(unsafe { (ump.set_midi_version)(client.handle, version.raw()) })?;
            client.version = version;
        }
        Ok(client)
    }

    /// Gives the client room for as many events waiting to be read as the
    /// kernel allows, 2,000, where it gives 200 unless asked: a SysEx
    /// takes one for each 28 bytes, so a dump that comes at once waits
    /// while the client's thread wakes, rather than being dropped. alsa-lib
    /// reads through a buffer of its own, which is made to hold as much, so
    /// a SysEx another app sends as one event, which the room may take
    /// whole, is never too long to read.
    pub(super) fn with_input_room(self) -> Client {
        // SAFETY: the open handle; a size the kernel refuses changes
        // nothing.
        unsafe { sys::snd_seq_set_client_pool_input(self.handle, ROOM) };
        let bytes = ROOM.saturating_mul(size_of::<sys::snd_seq_ump_event_t>());
        // SAFETY: the open handle; a buffer alsa-lib cannot make leaves the
        // one it has.
        unsafe { sys::snd_seq_set_input_buffer_size(self.handle, bytes) };
        self
    }

    /// Reads, of what comes, only what the port at `from` sends and what
    /// the System client announces: what another app sends straight to the
    /// client's port is passed over.
    pub(super) fn only_from(mut self, from: Address) -> Client {
        self.from = Some(from);
        self
    }

    /// Whether the client reads what came from `source`.
    fn hears(&self, source: sys::snd_seq_addr) -> bool {
        let source = Address {
            client: source.client,
            port: source.port,
        };
        self.from
            .is_none_or(|from| source == from || source == ANNOUNCE)
    }

    /// Whether the port at `at` is in the sequencer now.
    pub(super) fn has_port(&self, at: Address) -> bool {
        // Without the memory to ask, it is taken to be there.
        let Some(info) = PortInfo::new() else {
            return true;
        };
        // SAFETY: the open handle and the info the call fills.
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

    /// Makes a port of the client's own, `name`, with `caps`, and answers
    /// its number.
    pub(super) fn port(&self, name: &str, caps: c_uint) -> Result<u8, OpenError> {
        let name = CString::new(name.replace('\0', "")).unwrap_or_default();
        // SAFETY: the open handle and a name that lives through the call.
        let port = check(unsafe {
            sys::snd_seq_create_simple_port(
                self.handle,
                name.as_ptr(),
                caps,
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

    /// Reads every event waiting, handing each to `each`, until none is
    /// left. What an event lends lives until the next.
    pub(super) fn read(&mut self, parser: &mut Parser, mut each: impl FnMut(Incoming<'_>)) {
        loop {
            let event = if self.version == Version::Legacy {
                let mut event = ptr::null_mut();
                // SAFETY: the open handle; the event, if one came, is
                // alsa-lib's until the next read.
                match unsafe { sys::snd_seq_event_input(self.handle, &raw mut event) } {
                    OVERRUN => {
                        each(Incoming::Overrun);
                        continue;
                    }
                    code if code < 0 => return,
                    _ => {}
                }
                // SAFETY: alsa-lib's event, if one came, until the next read.
                if !event.is_null() && !self.hears(unsafe { (*event).source }) {
                    continue;
                }
                event.cast_const()
            } else {
                let Some(ump) = ump() else { return };
                let mut event = ptr::null_mut();
                // SAFETY: as for a legacy client, with alsa-lib's own
                // function, found by its name.
                match unsafe { (ump.event_input)(self.handle, &raw mut event) } {
                    OVERRUN => {
                        each(Incoming::Overrun);
                        continue;
                    }
                    code if code < 0 => return,
                    _ if event.is_null() => return,
                    _ => {}
                }
                // SAFETY: alsa-lib's event, until the next read.
                let ump_event = unsafe { &*event };
                if !self.hears(ump_event.source) {
                    continue;
                }
                if ump_event.flags & EVENT_UMP != 0 {
                    // SAFETY: a UMP event's data is its packet.
                    each(Incoming::Packet(unsafe { ump_event.__bindgen_anon_1.ump }));
                    continue;
                }
                // Any other event a UMP client reads has a legacy event's
                // shape: the same fields, then the same data.
                event.cast_const().cast::<sys::snd_seq_event_t>()
            };
            if event.is_null() {
                return;
            }
            // SAFETY: alsa-lib's event, until the next read, which comes
            // only after `each` is done with what it lends.
            each(unsafe { classify(event, parser) });
        }
    }

    /// Sends `bytes`, one whole MIDI 1.0 message but a SysEx, from `port`
    /// to every port subscribed to it, at once. A refusal is alsa-lib's
    /// error number.
    pub(super) fn send(
        &mut self,
        parser: &mut Parser,
        port: u8,
        bytes: &[u8],
    ) -> Result<(), c_int> {
        match parser.encode(bytes) {
            Some(event) => self.output(event, port),
            None => Ok(()),
        }
    }

    /// Sends `piece`, a SysEx or a piece of one, as it is, from `port` to
    /// every port subscribed to it, at once.
    pub(super) fn send_sysex(&mut self, port: u8, piece: &[u8]) -> Result<(), c_int> {
        // SAFETY: an event of zeros is a whole one, every field filled.
        let mut event: sys::snd_seq_event_t = unsafe { std::mem::zeroed() };
        event.type_ = EVENT_SYSEX;
        event.flags = LENGTH_VARIABLE;
        event.data.ext = sys::snd_seq_ev_ext {
            len: c_uint::try_from(piece.len()).unwrap_or(c_uint::MAX),
            ptr: piece.as_ptr().cast_mut().cast(),
        };
        self.output(event, port)
    }

    /// Sends `bytes`, one whole MIDI 1.0 message but a SysEx, from `port`
    /// to the port at `to` alone, subscribed or not: a test's other app.
    #[cfg(test)]
    pub(super) fn send_to(
        &mut self,
        parser: &mut Parser,
        port: u8,
        to: Address,
        bytes: &[u8],
    ) -> Result<(), c_int> {
        match parser.encode(bytes) {
            Some(event) => self.output_to(event, port, to),
            None => Ok(()),
        }
    }

    fn output(&mut self, event: sys::snd_seq_event_t, port: u8) -> Result<(), c_int> {
        let subscribers = Address {
            client: ADDRESS_SUBSCRIBERS,
            port: ADDRESS_UNKNOWN,
        };
        self.output_to(event, port, subscribers)
    }

    fn output_to(
        &mut self,
        mut event: sys::snd_seq_event_t,
        port: u8,
        to: Address,
    ) -> Result<(), c_int> {
        event.queue = QUEUE_DIRECT;
        event.source.port = port;
        event.dest = sys::snd_seq_addr {
            client: to.client,
            port: to.port,
        };
        // SAFETY: the open handle and an event that lives through the call;
        // a SysEx's points at bytes that do too, which alsa-lib only reads.
        let sent = unsafe { sys::snd_seq_event_output_direct(self.handle, &raw mut event) };
        if sent < 0 {
            Err(sent.saturating_neg())
        } else {
            Ok(())
        }
    }

    /// Sends `words`, one UMP packet, from `port` to every port subscribed
    /// to it, at once: a test's MIDI 2.0 device.
    #[cfg(test)]
    pub(super) fn send_packet(&mut self, port: u8, words: [u32; 4]) -> Result<(), c_int> {
        let ump = ump().ok_or(libc::ENOSYS)?;
        // SAFETY: an event of zeros is a whole one, every field filled.
        let mut event: sys::snd_seq_ump_event_t = unsafe { std::mem::zeroed() };
        event.flags = EVENT_UMP;
        event.queue = QUEUE_DIRECT;
        event.source.port = port;
        event.dest = sys::snd_seq_addr {
            client: ADDRESS_SUBSCRIBERS,
            port: ADDRESS_UNKNOWN,
        };
        event.__bindgen_anon_1.ump = words;
        // SAFETY: the open handle, a UMP client, and an event that lives
        // through the call.
        let sent = unsafe { (ump.event_output_direct)(self.handle, &raw mut event) };
        if sent < 0 {
            Err(sent.saturating_neg())
        } else {
            Ok(())
        }
    }

    /// Every port of every client the sequencer has now, in its order.
    pub(super) fn ports(&self) -> Vec<PortEntry> {
        let (Some(client), Some(port)) = (ClientInfo::new(), PortInfo::new()) else {
            return Vec::new();
        };
        let ump = ump();
        // SAFETY: an info alsa-lib made; -1 starts before the first client.
        unsafe { sys::snd_seq_client_info_set_client(client.0, -1) };
        let mut entries = Vec::new();
        // SAFETY: the open handle and the info the call fills.
        while unsafe { sys::snd_seq_query_next_client(self.handle, client.0) } >= 0 {
            // SAFETY: the info, filled; its name lives as long as it does.
            let (number, client_name, card) = unsafe {
                (
                    sys::snd_seq_client_info_get_client(client.0),
                    text(sys::snd_seq_client_info_get_name(client.0)),
                    sys::snd_seq_client_info_get_card(client.0),
                )
            };
            let version = ump.map_or(Version::Legacy, |ump| {
                // SAFETY: the info, filled; alsa-lib's own function.
                Version::from_raw(unsafe { (ump.info_midi_version)(client.0) })
            });
            let Ok(number) = u8::try_from(number) else {
                continue;
            };
            // SAFETY: the port info alsa-lib made; -1 starts before the
            // client's first port.
            unsafe {
                sys::snd_seq_port_info_set_client(port.0, c_int::from(number));
                sys::snd_seq_port_info_set_port(port.0, -1);
            }
            // SAFETY: the open handle and the info the call fills.
            while unsafe { sys::snd_seq_query_next_port(self.handle, port.0) } >= 0 {
                // SAFETY: the info, filled; its name lives as long as it
                // does.
                let (place, name, caps) = unsafe {
                    (
                        sys::snd_seq_port_info_get_port(port.0),
                        text(sys::snd_seq_port_info_get_name(port.0)),
                        sys::snd_seq_port_info_get_capability(port.0),
                    )
                };
                let Ok(place) = u8::try_from(place) else {
                    continue;
                };
                entries.push(PortEntry {
                    address: Address {
                        client: number,
                        port: place,
                    },
                    client_name: client_name.clone(),
                    name,
                    card: u32::try_from(card).ok(),
                    caps,
                    version,
                });
            }
        }
        entries
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: the open handle, closed once; its ports and subscriptions
        // go with it.
        unsafe { sys::snd_seq_close(self.handle) };
    }
}

/// What `event`, a legacy event, is.
///
/// # Safety
///
/// `event` is alsa-lib's, valid until the caller reads the next, after it
/// is done with what this lends.
unsafe fn classify(event: *const sys::snd_seq_event_t, parser: &mut Parser) -> Incoming<'_> {
    // SAFETY: as the caller says.
    let ev = unsafe { &*event };
    if (ev.source.client, ev.source.port) == (ANNOUNCE.client, ANNOUNCE.port) {
        // SAFETY: an announcement's data is the address it names.
        let at = unsafe { ev.data.addr };
        let at = Address {
            client: at.client,
            port: at.port,
        };
        return Incoming::Announced(match ev.type_ {
            EVENT_CLIENT_EXIT => Change::ClientLeft(at.client),
            EVENT_PORT_EXIT => Change::PortLeft(at),
            EVENT_CLIENT_START | EVENT_CLIENT_CHANGE | EVENT_PORT_START | EVENT_PORT_CHANGE => {
                Change::Listed
            }
            _ => Change::Other,
        });
    }
    if ev.type_ == EVENT_SYSEX && ev.flags & LENGTH_MASK == LENGTH_VARIABLE {
        // SAFETY: a variable-length event's data is where its bytes are.
        let ext = unsafe { ev.data.ext };
        if ext.ptr.is_null() {
            return Incoming::Other;
        }
        // SAFETY: `len` bytes at `ptr`, in alsa-lib's buffer until the next
        // read, as the caller says.
        let bytes = unsafe { std::slice::from_raw_parts(ext.ptr.cast::<u8>(), ext.len as usize) };
        return Incoming::Bytes(bytes);
    }
    parser
        .decode(event)
        .map_or(Incoming::Other, Incoming::Bytes)
}

/// alsa-lib's MIDI event parser: an event into MIDI 1.0's bytes, and bytes
/// into an event. It frees itself as it drops.
#[derive(Debug)]
pub(super) struct Parser {
    handle: *mut sys::snd_midi_event_t,
    /// The bytes of the last message decoded: the longest but a SysEx is
    /// three.
    bytes: [u8; 16],
}

// SAFETY: the parser is alsa-lib's plain state, used by the one thread that
// holds it.
unsafe impl Send for Parser {}

impl Parser {
    /// A parser, or `None` where alsa-lib had no memory for it.
    pub(super) fn new() -> Option<Parser> {
        let mut handle = ptr::null_mut();
        // SAFETY: the handle is written by the call; this parser encodes no
        // SysEx, so its room is the longest other message.
        if unsafe { sys::snd_midi_event_new(16, &raw mut handle) } < 0 || handle.is_null() {
            return None;
        }
        // SAFETY: the parser made: every message it decodes has its status.
        unsafe { sys::snd_midi_event_no_status(handle, 1) };
        Some(Parser {
            handle,
            bytes: [0; 16],
        })
    }

    /// The bytes of `event`, if it is a MIDI message.
    fn decode(&mut self, event: *const sys::snd_seq_event_t) -> Option<&[u8]> {
        // SAFETY: the parser, the room of `bytes`, and an event the caller
        // keeps valid through the call.
        let length = unsafe {
            sys::snd_midi_event_decode(
                self.handle,
                self.bytes.as_mut_ptr(),
                self.bytes.len() as c_long,
                event,
            )
        };
        let length = usize::try_from(length).ok().filter(|&length| length > 0)?;
        self.bytes.get(..length)
    }

    /// The event `bytes` make, one whole message but a SysEx.
    fn encode(&mut self, bytes: &[u8]) -> Option<sys::snd_seq_event_t> {
        // SAFETY: an event of zeros is a whole one, every field filled.
        let mut event: sys::snd_seq_event_t = unsafe { std::mem::zeroed() };
        event.type_ = EVENT_NONE;
        // SAFETY: the parser, then the bytes and the event, alive through
        // the calls; it starts each message from no running status.
        let used = unsafe {
            sys::snd_midi_event_reset_encode(self.handle);
            sys::snd_midi_event_encode(
                self.handle,
                bytes.as_ptr(),
                c_long::try_from(bytes.len()).unwrap_or(0),
                &raw mut event,
            )
        };
        (used > 0 && event.type_ != EVENT_NONE).then_some(event)
    }
}

impl Drop for Parser {
    fn drop(&mut self) {
        // SAFETY: the parser made, freed once.
        unsafe { sys::snd_midi_event_free(self.handle) };
    }
}

/// A client info alsa-lib made, freed as it drops.
struct ClientInfo(*mut sys::snd_seq_client_info_t);

impl ClientInfo {
    fn new() -> Option<ClientInfo> {
        let mut info = ptr::null_mut();
        // SAFETY: the pointer is written by the call.
        (unsafe { sys::snd_seq_client_info_malloc(&raw mut info) } >= 0 && !info.is_null())
            .then_some(ClientInfo(info))
    }
}

impl Drop for ClientInfo {
    fn drop(&mut self) {
        // SAFETY: the info made, freed once.
        unsafe { sys::snd_seq_client_info_free(self.0) };
    }
}

/// A port info alsa-lib made, freed as it drops.
struct PortInfo(*mut sys::snd_seq_port_info_t);

impl PortInfo {
    fn new() -> Option<PortInfo> {
        let mut info = ptr::null_mut();
        // SAFETY: the pointer is written by the call.
        (unsafe { sys::snd_seq_port_info_malloc(&raw mut info) } >= 0 && !info.is_null())
            .then_some(PortInfo(info))
    }
}

impl Drop for PortInfo {
    fn drop(&mut self) {
        // SAFETY: the info made, freed once.
        unsafe { sys::snd_seq_port_info_free(self.0) };
    }
}
