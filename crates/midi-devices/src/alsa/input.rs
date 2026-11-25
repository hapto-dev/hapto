//! One input, open: a client of the app's own subscribed to the device's
//! port, and the thread that reads what it sends into the input's ring.
//!
//! The client speaks the device's own MIDI version, so the kernel
//! translates nothing: a MIDI 1.0 device's events become bytes, read with
//! `ByteStream` as WinMM's are, and the reader upgrades them by Hapto's
//! own rules; a MIDI 2.0 device's packets go into the ring as they came,
//! on their own groups.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use hapto_ump::{ByteStream, MessageType};

use super::Listed;
use super::seq::{
    ANNOUNCE, Address, CAP_NO_EXPORT, CAP_SUBS_WRITE, CAP_WRITE, Client, Incoming, Parser, Streams,
    Version,
};
use super::signal::{Signal, wait};
use crate::Instant;
use crate::error::OpenError;
use crate::id::{Device, Direction};
use crate::ring::Producer;

/// What the input shares with its thread.
#[derive(Debug)]
struct Shared {
    /// Tells the thread to end.
    stopping: AtomicBool,
    /// Set when the System client says the device's port went: it left.
    left: AtomicBool,
    /// Wakes the thread to end.
    wake: Signal,
}

/// An input, reading into its ring while open. It closes as it drops.
pub(crate) struct Input {
    shared: Arc<Shared>,
    /// Hands the ring's producer back as it ends; `None` once closed.
    thread: Option<JoinHandle<Producer>>,
}

impl fmt::Debug for Input {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Input")
            .field("open", &self.thread.is_some())
            .finish_non_exhaustive()
    }
}

impl Input {
    /// Opens `device` to read into `producer`'s ring. On a refusal the
    /// producer comes back, unless the thread that would read could not
    /// start, which took it.
    pub(crate) fn open(
        device: &Device,
        producer: Producer,
    ) -> Result<Input, (OpenError, Option<Producer>)> {
        let Some(listed) = super::find(Direction::Input, device.id()) else {
            return Err((OpenError::Gone, Some(producer)));
        };
        let (client, parser) = match subscribe(&listed, device) {
            Ok(opened) => opened,
            Err(why) => return Err((why, Some(producer))),
        };
        let shared = Arc::new(Shared {
            stopping: AtomicBool::new(false),
            left: AtomicBool::new(false),
            wake: Signal::new(),
        });
        // Without an eventfd the thread could not be woken to end.
        if shared.wake.poll_fd().is_none() {
            return Err((OpenError::NoMemory, Some(producer)));
        }
        let thread_shared = Arc::clone(&shared);
        let at = listed.address;
        let started = thread::Builder::new()
            .name("hapto midi in".to_owned())
            .spawn(move || read(client, parser, at, producer, &thread_shared));
        match started {
            Ok(thread) => Ok(Input {
                shared,
                thread: Some(thread),
            }),
            Err(_) => Err((OpenError::NoMemory, None)),
        }
    }

    /// Whether the device left while open.
    pub(crate) fn has_left(&self) -> bool {
        self.shared.left.load(Ordering::Acquire)
    }

    /// Closes the device and gives its ring's producer back.
    pub(crate) fn close(mut self) -> Option<Producer> {
        self.shut()
    }

    /// Ends the thread, which closes the client, and takes the producer
    /// back from it.
    fn shut(&mut self) -> Option<Producer> {
        let thread = self.thread.take()?;
        self.shared.stopping.store(true, Ordering::Release);
        self.shared.wake.set();
        thread.join().ok()
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        self.shut();
    }
}

/// A client of the device's own MIDI version, its port subscribed to the
/// device's and to the System client's announcements, so it hears the
/// device leave. Where the kernel refuses that version, a MIDI 1.0 client,
/// which the kernel translates for.
fn subscribe(listed: &Listed, device: &Device) -> Result<(Client, Parser), OpenError> {
    let client = Client::open(Streams::Input, listed.version)
        .or_else(|_| Client::open(Streams::Input, Version::Legacy))?
        .with_input_room();
    let name = format!("in: {}", device.name());
    let port = client.port(&name, CAP_WRITE | CAP_SUBS_WRITE | CAP_NO_EXPORT)?;
    client.connect_from(port, listed.address)?;
    client.connect_from(port, ANNOUNCE)?;
    let parser = Parser::new().ok_or(OpenError::NoMemory)?;
    Ok((client, parser))
}

/// The input's thread: it reads what the device at `device` sends into
/// `producer`'s ring, stamped as it comes, until the input ends it, and
/// hands the producer back.
fn read(
    mut client: Client,
    mut parser: Parser,
    device: Address,
    mut producer: Producer,
    shared: &Shared,
) -> Producer {
    // Every MIDI 1.0 device on group 1, merged as a host merges them.
    let mut stream = ByteStream::new(0);
    let mut fds = client.poll_fds();
    fds.extend(shared.wake.poll_fd());
    while !shared.stopping.load(Ordering::Acquire) {
        wait(&mut fds, None);
        shared.wake.clear();
        let at = Instant::now();
        client.read(&mut parser, |incoming| match incoming {
            Incoming::Packet(words) => producer.push(at, packet(words)),
            Incoming::Bytes(bytes) => stream.read(bytes, |words| producer.push(at, words)),
            Incoming::Announced(change) if change.ends(device) => {
                shared.left.store(true, Ordering::Release);
            }
            // Some of what came was dropped: a SysEx under way has a hole,
            // so it ends here rather than reach the block as if whole.
            Incoming::Overrun => stream.cut(|words| producer.push(at, words)),
            Incoming::Announced(_) | Incoming::Other => {}
        });
    }
    // A SysEx the close cut short ends there, so the reader does not hold
    // it, and the real-time messages behind it, for good.
    stream.cut(|words| producer.push(Instant::now(), words));
    producer
}

/// The packet `words` hold, past its length zeroed.
fn packet(mut words: [u32; 4]) -> [u32; 4] {
    let length = MessageType::of(words[0]).words();
    for word in words.iter_mut().skip(length) {
        *word = 0;
    }
    words
}
