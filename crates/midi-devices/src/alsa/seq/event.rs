//! The events that a client reads and sends.
//!
//! alsa-lib's events are sorted into what the backend acts on, and its
//! parser converts MIDI 1.0's bytes to events and back.

use std::ffi::{c_int, c_long, c_uint};
use std::ptr;

use alsa_sys as sys;

use super::{ANNOUNCE, Address, Client};

// The header's macros, which the bindings leave out.
const QUEUE_DIRECT: u8 = 253;
const LENGTH_MASK: u8 = 3 << 2;
const LENGTH_VARIABLE: u8 = 1 << 2;
const EVENT_UMP: u8 = 1 << 5;
const EVENT_SYSEX: u8 = 130;
const EVENT_NONE: u8 = 255;
const EVENT_CLIENT_EXIT: u8 = 61;
const EVENT_CLIENT_CHANGE: u8 = 62;
const EVENT_PORT_START: u8 = 63;
const EVENT_PORT_EXIT: u8 = 64;
const EVENT_PORT_CHANGE: u8 = 65;

/// The destination for every port subscribed to the sender's.
const SUBSCRIBERS: Address = Address {
    client: 254,
    port: 253,
};

/// What a read returns once the client's room has run out.
const OVERRUN: c_int = -libc::ENOSPC;

/// The room that the parser decodes into, enough for any message but a
/// SysEx.
const PARSER_ROOM: usize = 16;

/// One event read from a client.
pub(crate) enum Incoming<'a> {
    /// A UMP packet's words, from a device that speaks UMP.
    Packet([u32; 4]),
    /// MIDI 1.0's bytes: a whole message, or a piece of a SysEx.
    Bytes(&'a [u8]),
    /// The System client's announcement about a client or port.
    Announced(Change),
    /// The client's room ran out, so the kernel dropped what came meanwhile.
    Overrun,
    /// Anything else.
    Other,
}

/// What the System client announced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    /// A client changed, or a port came, changed or went, so the list may
    /// differ.
    Listed,
    /// A client left, with every port it had.
    ClientLeft(u8),
    /// A port left.
    PortLeft(Address),
    /// Nothing that the list shows: a subscription that came or went, or a
    /// new client, whose ports are announced as they come.
    Other,
}

impl Change {
    /// Returns whether the port at `port` is gone.
    pub(crate) fn ends(self, port: Address) -> bool {
        match self {
            Change::ClientLeft(client) => client == port.client,
            Change::PortLeft(left) => left == port,
            Change::Listed | Change::Other => false,
        }
    }

    /// Returns whether the list may differ.
    pub(crate) fn lists(self) -> bool {
        !matches!(self, Change::Other)
    }
}

/// Why the sequencer refused a send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SendError {
    /// The device has no room now, but takes the event once it drains.
    Full,
    /// Refused for good.
    Refused,
}

/// Returns what a send's alsa-lib `code` means.
fn sent(code: c_int) -> Result<(), SendError> {
    if code >= 0 {
        Ok(())
    } else if code == -libc::ENOMEM || code == -libc::EAGAIN {
        Err(SendError::Full)
    } else {
        Err(SendError::Refused)
    }
}

/// The next event that alsa-lib has for a client.
enum Next {
    /// No event is waiting.
    Empty,
    /// The client's room ran out.
    Overrun,
    /// A UMP packet, and the port that sent it.
    Packet(Address, [u32; 4]),
    /// Any other event, in a legacy event's shape, alsa-lib's until the
    /// next read.
    Event(*const sys::snd_seq_event_t),
}

impl Client {
    /// Reads every waiting event, passing each to `each`.
    ///
    /// The bytes that an [`Incoming`] borrows live only until the next event
    /// is read.
    pub(crate) fn read(&mut self, parser: &mut Parser, mut each: impl FnMut(Incoming<'_>)) {
        loop {
            match self.next_event() {
                Next::Empty => return,
                Next::Overrun => {
                    each(Incoming::Overrun);
                    if let Some(device) = self.device
                        && !self.has_port(device)
                    {
                        each(Incoming::Announced(Change::PortLeft(device)));
                    }
                }
                Next::Packet(source, words) => {
                    if self.hears(source) {
                        each(Incoming::Packet(words));
                    }
                }
                Next::Event(event) => {
                    // SAFETY: alsa-lib's event, until the next read, which
                    // comes only after `each` is done with what it borrows.
                    let source = Address::from(unsafe { (*event).source });
                    if self.hears(source) {
                        // SAFETY: as above.
                        each(unsafe { classify(event, parser) });
                    }
                }
            }
            // With alsa-lib's buffer empty, another read would only call the
            // kernel and find nothing, and what comes later wakes the next
            // wait.
            // SAFETY: the open handle, and with `fetch_sequencer` at 0 the
            // call only counts what alsa-lib holds.
            if unsafe { sys::snd_seq_event_input_pending(self.handle, 0) } <= 0 {
                return;
            }
        }
    }

    /// Returns the next event that alsa-lib has for the client.
    fn next_event(&mut self) -> Next {
        let Some(ump) = self.ump else {
            let mut event = ptr::null_mut();
            // SAFETY: the open handle, and the event, if one came, is
            // alsa-lib's until the next read.
            return match unsafe { sys::snd_seq_event_input(self.handle, &raw mut event) } {
                OVERRUN => Next::Overrun,
                code if code < 0 || event.is_null() => Next::Empty,
                _ => Next::Event(event.cast_const()),
            };
        };
        let mut event: *mut sys::snd_seq_ump_event_t = ptr::null_mut();
        // SAFETY: as for a MIDI 1.0 client, with alsa-lib's own function,
        // found by its name.
        match unsafe { (ump.event_input)(self.handle, &raw mut event) } {
            OVERRUN => Next::Overrun,
            code if code < 0 || event.is_null() => Next::Empty,
            _ => {
                // SAFETY: alsa-lib's event, until the next read.
                let ump_event = unsafe { &*event };
                if ump_event.flags & EVENT_UMP == 0 {
                    // Any other event that a UMP client reads has a legacy
                    // event's fields and data, in the same layout.
                    return Next::Event(event.cast_const().cast());
                }
                // SAFETY: a UMP event's data is its packet.
                let words = unsafe { ump_event.__bindgen_anon_1.ump };
                Next::Packet(ump_event.source.into(), words)
            }
        }
    }

    /// Returns whether the client reads what came from `source`.
    fn hears(&self, source: Address) -> bool {
        self.device
            .is_none_or(|device| source == device || source == ANNOUNCE)
    }

    /// Sends `bytes`, one whole MIDI 1.0 message other than a SysEx, from
    /// `port` to every port subscribed to it, at once.
    pub(crate) fn send(
        &mut self,
        parser: &mut Parser,
        port: u8,
        bytes: &[u8],
    ) -> Result<(), SendError> {
        self.send_to(parser, port, SUBSCRIBERS, bytes)
    }

    /// Sends `bytes`, as [`send`](Self::send) does, to the port at `to`
    /// alone, subscribed or not.
    pub(crate) fn send_to(
        &mut self,
        parser: &mut Parser,
        port: u8,
        to: Address,
        bytes: &[u8],
    ) -> Result<(), SendError> {
        match parser.encode(bytes) {
            Some(event) => self.output(event, port, to),
            None => Ok(()),
        }
    }

    /// Sends `piece`, a SysEx or a piece of one, as it is, from `port` to
    /// every port subscribed to it, at once.
    pub(crate) fn send_sysex(&mut self, port: u8, piece: &[u8]) -> Result<(), SendError> {
        // SAFETY: all zeros is a valid event.
        let mut event: sys::snd_seq_event_t = unsafe { std::mem::zeroed() };
        event.type_ = EVENT_SYSEX;
        event.flags = LENGTH_VARIABLE;
        event.data.ext = sys::snd_seq_ev_ext {
            len: c_uint::try_from(piece.len()).unwrap_or(c_uint::MAX),
            ptr: piece.as_ptr().cast_mut().cast(),
        };
        self.output(event, port, SUBSCRIBERS)
    }

    /// Sends `words`, one UMP packet, from `port` to every port subscribed
    /// to it, at once.
    ///
    /// A test uses it to play a MIDI 2.0 device.
    #[cfg(test)]
    pub(crate) fn send_packet(&mut self, port: u8, words: [u32; 4]) -> Result<(), SendError> {
        let ump = self.ump.ok_or(SendError::Refused)?;
        // SAFETY: all zeros is a valid event.
        let mut event: sys::snd_seq_ump_event_t = unsafe { std::mem::zeroed() };
        event.flags = EVENT_UMP;
        event.queue = QUEUE_DIRECT;
        event.source.port = port;
        event.dest = SUBSCRIBERS.into();
        event.__bindgen_anon_1.ump = words;
        // SAFETY: the open handle, a UMP client's, and an event that lives
        // through the call.
        sent(unsafe { (ump.event_output_direct)(self.handle, &raw mut event) })
    }

    fn output(
        &mut self,
        mut event: sys::snd_seq_event_t,
        port: u8,
        to: Address,
    ) -> Result<(), SendError> {
        event.queue = QUEUE_DIRECT;
        event.source.port = port;
        event.dest = to.into();
        // SAFETY: the open handle and an event that lives through the call.
        // A SysEx event points at bytes that do too, which alsa-lib only
        // reads.
        sent(unsafe { sys::snd_seq_event_output_direct(self.handle, &raw mut event) })
    }
}

/// Sorts `event`, a legacy event, into an [`Incoming`].
///
/// # Safety
///
/// `event` is alsa-lib's, valid until the caller reads the next event,
/// which it does only once done with what this returns.
unsafe fn classify(event: *const sys::snd_seq_event_t, parser: &mut Parser) -> Incoming<'_> {
    // SAFETY: as the caller guarantees.
    let ev = unsafe { &*event };
    if Address::from(ev.source) == ANNOUNCE {
        // SAFETY: an announcement's data is the address that it names.
        let at = Address::from(unsafe { ev.data.addr });
        return Incoming::Announced(match ev.type_ {
            EVENT_CLIENT_EXIT => Change::ClientLeft(at.client),
            EVENT_PORT_EXIT => Change::PortLeft(at),
            EVENT_CLIENT_CHANGE | EVENT_PORT_START | EVENT_PORT_CHANGE => Change::Listed,
            _ => Change::Other,
        });
    }
    if ev.type_ == EVENT_SYSEX && ev.flags & LENGTH_MASK == LENGTH_VARIABLE {
        // SAFETY: a variable-length event's data points at its bytes.
        let ext = unsafe { ev.data.ext };
        if ext.ptr.is_null() {
            return Incoming::Other;
        }
        // SAFETY: `len` bytes at `ptr`, in alsa-lib's buffer until the next
        // read, as the caller guarantees.
        let bytes = unsafe { std::slice::from_raw_parts(ext.ptr.cast::<u8>(), ext.len as usize) };
        return Incoming::Bytes(bytes);
    }
    parser
        .decode(event)
        .map_or(Incoming::Other, Incoming::Bytes)
}

/// alsa-lib's MIDI event parser, which converts an event to MIDI 1.0's
/// bytes and back, and is freed when it drops.
#[derive(Debug)]
pub(crate) struct Parser {
    handle: *mut sys::snd_midi_event_t,
    /// The bytes of the last message decoded.
    bytes: [u8; PARSER_ROOM],
}

// SAFETY: the parser is alsa-lib's plain state, used by the one thread that
// holds it.
unsafe impl Send for Parser {}

impl Parser {
    /// Creates a parser, or returns `None` if alsa-lib has no memory for it.
    pub(crate) fn new() -> Option<Parser> {
        let mut handle = ptr::null_mut();
        // SAFETY: the call writes the handle. This parser encodes no SysEx,
        // so its room holds the longest message that it encodes.
        if unsafe { sys::snd_midi_event_new(PARSER_ROOM, &raw mut handle) } < 0 || handle.is_null()
        {
            return None;
        }
        // SAFETY: the allocated parser, set so every decoded message has its
        // status.
        unsafe { sys::snd_midi_event_no_status(handle, 1) };
        Some(Parser {
            handle,
            bytes: [0; PARSER_ROOM],
        })
    }

    /// Returns the bytes of `event`, if it is a MIDI message.
    fn decode(&mut self, event: *const sys::snd_seq_event_t) -> Option<&[u8]> {
        // SAFETY: the parser, the room in `bytes`, and an event that the
        // caller keeps valid through the call.
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

    /// Converts `bytes`, one whole message other than a SysEx, to an event.
    fn encode(&mut self, bytes: &[u8]) -> Option<sys::snd_seq_event_t> {
        // SAFETY: all zeros is a valid event.
        let mut event: sys::snd_seq_event_t = unsafe { std::mem::zeroed() };
        event.type_ = EVENT_NONE;
        // SAFETY: the parser, and the bytes and the event, alive through the
        // calls. The reset starts each message with no running status.
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
        // SAFETY: the allocated parser, freed once.
        unsafe { sys::snd_midi_event_free(self.handle) };
    }
}
