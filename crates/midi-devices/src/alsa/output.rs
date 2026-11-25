//! One output, open: a client of the app's own subscribed to by the
//! device's port, and the thread that sends what the audio thread hands it,
//! each message at its time.
//!
//! The audio thread never calls alsa-lib. It pushes packets into the
//! output's ring, each stamped with when it is due, and sets the output's
//! signal; the output's own thread waits until the next is due or the
//! signal wakes it, then sends what is due, at once. The writer has
//! downgraded every packet to MIDI 1.0, so the client is a MIDI 1.0 one.
//!
//! The kernel hands a card's port each event whole and drops one that does
//! not fit the room left in the device's buffer, a page by default. So a
//! SysEx goes out in pieces, and everything goes out paced: no more than
//! half a page ahead of what a MIDI cable would have carried by then.
//!
//! A close sends nothing. The sequencer shares a device among apps, so
//! turning its notes off would cut another app's too: the notes an app
//! leaves on are its own to end, as through every Linux MIDI library.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use hapto_ump::{Bytes, Event, MessageType};

use super::seq::{
    ANNOUNCE, Address, CAP_NO_EXPORT, CAP_READ, CAP_SUBS_READ, CAP_WRITE, Client, Incoming, Parser,
    Streams, Version,
};
use super::signal::{Signal, wait};
use crate::Instant;
use crate::error::OpenError;
use crate::id::{Device, Direction};
use crate::outputs::Sysex;
use crate::ring::Consumer;

/// How fast a port drains what it is sent, at most as far as anyone can
/// tell: a MIDI cable's 31,250 baud, ten bits a byte.
const BYTES_PER_SECOND: f64 = 3125.0;
/// How far ahead of the cable an output may send: half the page the kernel
/// gives a card's port.
const AHEAD: f64 = 2048.0;
/// The longest piece of a SysEx sent as one event.
const PIECE: usize = 256;

/// What the output shares with its thread.
#[derive(Debug)]
struct Shared {
    /// Tells the thread to end.
    stopping: AtomicBool,
    /// Set when the System client says the device's port went: it left.
    left: AtomicBool,
}

/// An output, sending what its ring holds while open. It closes as it
/// drops.
pub(crate) struct Output {
    shared: Arc<Shared>,
    signal: Arc<Signal>,
    /// Hands the ring's consumer back as it ends; `None` once closed.
    thread: Option<JoinHandle<Consumer>>,
}

impl fmt::Debug for Output {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Output")
            .field("open", &self.thread.is_some())
            .finish_non_exhaustive()
    }
}

impl Output {
    /// Opens `device` to send what `consumer`'s ring holds, its thread
    /// woken by `signal`. On a refusal the consumer comes back, unless the
    /// thread that would send could not start, which took it.
    pub(crate) fn open(
        device: &Device,
        consumer: Consumer,
        signal: Arc<Signal>,
    ) -> Result<Output, (OpenError, Option<Consumer>)> {
        let Some(listed) = super::find(Direction::Output, device.id()) else {
            return Err((OpenError::Gone, Some(consumer)));
        };
        // Without an eventfd the thread could not be woken.
        if signal.poll_fd().is_none() {
            return Err((OpenError::NoMemory, Some(consumer)));
        }
        let sender = match Sender::subscribe(listed.address, device) {
            Ok(sender) => sender,
            Err(why) => return Err((why, Some(consumer))),
        };
        let shared = Arc::new(Shared {
            stopping: AtomicBool::new(false),
            left: AtomicBool::new(false),
        });
        let (thread_shared, thread_signal) = (Arc::clone(&shared), Arc::clone(&signal));
        // Read from here, so the next block's MIDI is the output's however
        // soon its thread starts, until the thread ends however it ends.
        let reading = consumer.reading();
        let started = thread::Builder::new()
            .name("hapto midi out".to_owned())
            .spawn(move || {
                let _reading = reading;
                send(sender, &thread_shared, &thread_signal, consumer)
            });
        match started {
            Ok(thread) => Ok(Output {
                shared,
                signal,
                thread: Some(thread),
            }),
            Err(_) => Err((OpenError::NoMemory, None)),
        }
    }

    /// Whether the device left while open.
    pub(crate) fn has_left(&self) -> bool {
        self.shared.left.load(Ordering::Acquire)
    }

    /// Closes the device and gives its ring's consumer back, with what it
    /// had not sent dropped.
    pub(crate) fn close(mut self) -> Option<Consumer> {
        self.shut()
    }

    /// Ends the thread, which closes the client, and takes the consumer
    /// back from it.
    fn shut(&mut self) -> Option<Consumer> {
        let thread = self.thread.take()?;
        self.shared.stopping.store(true, Ordering::Release);
        self.signal.set();
        thread.join().ok()
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.shut();
    }
}

/// What the output's thread does next.
#[derive(Debug, PartialEq)]
enum Next {
    /// Goes on at once.
    Now,
    /// Waits for the signal.
    Signal,
    /// Waits this long, or for the signal.
    After(Duration),
}

/// The output's thread: it sends each packet once its time comes, until
/// the output ends it, then hands the ring back.
fn send(mut sender: Sender, shared: &Shared, signal: &Signal, mut consumer: Consumer) -> Consumer {
    let mut sysex = Sysex::new();
    // How much of a SysEx joined has gone out, while some has not.
    let mut sent: Option<usize> = None;
    let mut fds = sender.client.poll_fds();
    fds.extend(signal.poll_fd());
    while !shared.stopping.load(Ordering::Acquire) {
        if sender.has_left() {
            shared.left.store(true, Ordering::Release);
        }
        let now = Instant::now();
        let next = match sent {
            Some(from) => sender.piece(&sysex, &mut sent, from, now, &mut consumer),
            None => match consumer.peek() {
                None => Next::Signal,
                Some((due, _)) if due > now => Next::After(due.since(now)),
                Some((_, words)) => sender.packet(words, now, &mut consumer, &mut sysex, &mut sent),
            },
        };
        match next {
            Next::Now => continue,
            Next::Signal => wait(&mut fds, None),
            Next::After(timeout) => wait(&mut fds, Some(timeout)),
        }
        signal.clear();
    }
    consumer
}

/// What the output's thread sends with.
struct Sender {
    client: Client,
    parser: Parser,
    port: u8,
    device: Address,
    pace: Pace,
}

impl Sender {
    /// A MIDI 1.0 client whose port the device at `device` subscribes to,
    /// and which hears the System client's announcements, so it hears the
    /// device leave.
    fn subscribe(device: Address, opened: &Device) -> Result<Sender, OpenError> {
        let client = Client::open(Streams::Duplex, Version::Legacy)?;
        let name = format!("out: {}", opened.name());
        let port = client.port(&name, CAP_READ | CAP_SUBS_READ | CAP_WRITE | CAP_NO_EXPORT)?;
        client.connect_to(port, device)?;
        client.connect_from(port, ANNOUNCE)?;
        let parser = Parser::new().ok_or(OpenError::NoMemory)?;
        Ok(Sender {
            client,
            parser,
            port,
            device,
            pace: Pace::new(),
        })
    }

    /// Whether the System client said the device went, since the last look.
    fn has_left(&mut self) -> bool {
        let device = self.device;
        let mut left = false;
        self.client.read(&mut self.parser, |incoming| {
            if let Incoming::Announced(change) = incoming
                && change.ends(device)
            {
                left = true;
            }
        });
        left
    }

    /// Sends the packet `words` at the ring's head, or joins it into
    /// `sysex`, and takes it from the ring once done with it.
    fn packet(
        &mut self,
        words: [u32; 4],
        now: Instant,
        consumer: &mut Consumer,
        sysex: &mut Sysex,
        sent: &mut Option<usize>,
    ) -> Next {
        // The writer pushes only what MIDI 1.0's bytes carry.
        let Some(bytes) = Bytes::of(words) else {
            consumer.pop();
            return Next::Now;
        };
        let event = Event::new(0, words);
        if event.message_type() == MessageType::SYSEX7 {
            consumer.pop();
            let joined = sysex.join(&event, consumer.dropped());
            for _ in 0..joined.dropped {
                consumer.count_dropped();
            }
            if joined.whole {
                *sent = Some(0);
            }
            return Next::Now;
        }
        let next = self.out(&bytes, false, now);
        match next {
            Ok(Next::Now) => {
                consumer.pop();
                Next::Now
            }
            Ok(wait) => wait,
            Err(()) => {
                consumer.pop();
                consumer.count_dropped();
                Next::Now
            }
        }
    }

    /// Sends the next piece of the SysEx joined, from `from`.
    fn piece(
        &mut self,
        sysex: &Sysex,
        sent: &mut Option<usize>,
        from: usize,
        now: Instant,
        consumer: &mut Consumer,
    ) -> Next {
        let rest = sysex.bytes.get(from..).unwrap_or(&[]);
        let piece = rest.get(..PIECE).unwrap_or(rest);
        match self.out(piece, true, now) {
            Ok(Next::Now) => {
                let next = from.saturating_add(piece.len());
                *sent = (next < sysex.bytes.len()).then_some(next);
                Next::Now
            }
            Ok(wait) => wait,
            Err(()) => {
                consumer.count_dropped();
                *sent = None;
                Next::Now
            }
        }
    }

    /// Sends `bytes`, a whole message, or a piece of a SysEx if `sysex`,
    /// once the pace allows it: `Ok(Next::Now)` once it went, how long to
    /// wait while the device has no room yet, and an error for a send
    /// refused for good.
    fn out(&mut self, bytes: &[u8], sysex: bool, now: Instant) -> Result<Next, ()> {
        if bytes.is_empty() {
            return Ok(Next::Now);
        }
        if let Some(wait) = self.pace.room_in(bytes.len(), now) {
            return Ok(Next::After(wait));
        }
        let sent = if sysex {
            self.client.send_sysex(self.port, bytes)
        } else {
            self.client.send(&mut self.parser, self.port, bytes)
        };
        match sent {
            Ok(()) => {
                self.pace.sent(bytes.len());
                Ok(Next::Now)
            }
            // The device's buffer is full after all: it waits for room and
            // goes again.
            Err(libc::ENOMEM | libc::EAGAIN) => {
                self.pace.fill(now);
                Ok(Next::After(
                    self.pace.room_in(bytes.len(), now).unwrap_or_default(),
                ))
            }
            Err(_) => Err(()),
        }
    }
}

/// What a port has been sent and has not drained yet, as a cable would
/// drain it.
#[derive(Debug)]
struct Pace {
    held: f64,
    at: Instant,
}

impl Pace {
    fn new() -> Pace {
        Pace {
            held: 0.0,
            at: Instant::now(),
        }
    }

    /// How long from `now` until `bytes` more fit: `None` if they fit now.
    /// An empty port takes anything.
    fn room_in(&mut self, bytes: usize, now: Instant) -> Option<Duration> {
        self.drain(now);
        let over = self.held + bytes as f64 - AHEAD;
        (self.held > 0.0 && over > 0.0).then(|| Duration::from_secs_f64(over / BYTES_PER_SECOND))
    }

    fn sent(&mut self, bytes: usize) {
        self.held += bytes as f64;
    }

    /// Takes the port as full at `now`.
    fn fill(&mut self, now: Instant) {
        self.drain(now);
        self.held = self.held.max(AHEAD);
    }

    fn drain(&mut self, now: Instant) {
        let gone = now.since(self.at).as_secs_f64() * BYTES_PER_SECOND;
        self.held = (self.held - gone).max(0.0);
        self.at = self.at.max(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pace_lets_half_a_page_ahead_of_a_cable_and_no_more() {
        let start = Instant::from_nanos(1_000_000_000);
        let mut pace = Pace {
            held: 0.0,
            at: start,
        };
        assert_eq!(
            pace.room_in(4096, start),
            None,
            "an empty port takes anything"
        );
        pace.sent(4096);
        // 2,049 bytes over: 2,049 of 3,125 bytes a second.
        assert_eq!(
            pace.room_in(1, start),
            Some(Duration::from_secs_f64(2049.0 / 3125.0))
        );
        // A second on, 971 bytes are held, and 1,077 more fit.
        let later = start.after(Duration::from_secs(1));
        assert_eq!(pace.room_in(1077, later), None);
        assert!(pace.room_in(1078, later).is_some());
    }

    #[test]
    fn a_device_that_says_it_is_full_is_given_time_to_drain() {
        let start = Instant::from_nanos(1_000_000_000);
        let mut pace = Pace {
            held: 0.0,
            at: start,
        };
        pace.sent(3);
        pace.fill(start);
        assert_eq!(
            pace.room_in(256, start),
            Some(Duration::from_secs_f64(256.0 / 3125.0))
        );
    }
}
