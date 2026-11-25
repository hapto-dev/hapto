//! An open output: a sequencer client of the app's own, its port connected
//! to the device's, and the thread that sends the device what the audio
//! thread hands it, each message at its time.
//!
//! The audio thread never calls alsa-lib. It pushes packets into the
//! output's ring, each stamped with when it is due, and sets the output's
//! signal. The output's own thread waits until the next packet is due or
//! the signal wakes it, then sends what is due as fast as the pace below
//! allows. The writer has downgraded every packet to MIDI 1.0, so the
//! client is a MIDI 1.0 one.
//!
//! The kernel hands a card's port each event whole, and drops an event that
//! does not fit the room left in the device's buffer, a page by default. So
//! a SysEx goes out in pieces, and everything goes out paced: no more than
//! half a page ahead of what a MIDI cable would have carried by then.
//!
//! A close sends nothing. The sequencer shares a device among apps, so
//! turning its notes off would cut off another app's notes too: as with
//! midir and `RtMidi`, ending the notes an app leaves on is the app's own
//! job.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use hapto_ump::{Bytes, Event, MessageType};

use super::seq::{ANNOUNCE, Address, Caps, Client, Incoming, Parser, SendError, Streams, Version};
use super::signal::Signal;
use super::worker::{Waiter, Worker};
use crate::Instant;
use crate::error::OpenError;
use crate::id::{Device, Direction};
use crate::outputs::OutgoingSysex;
use crate::ring::{Consumer, Reading};

/// The rate a port is taken to drain what it is sent, since the backend
/// cannot tell a port's own: a MIDI cable's 31,250 baud, ten bits a byte.
const BYTES_PER_SECOND: f64 = 3125.0;
/// How far ahead of the cable an output may send: half the page the kernel
/// gives a card's port.
const AHEAD: f64 = 2048.0;
/// The longest piece of a SysEx sent as one event.
const PIECE: usize = 256;

/// An output, sending what its ring holds while open. It closes as it
/// drops.
#[derive(Debug)]
pub(crate) struct Output {
    /// Set once the device's port is gone.
    left: Arc<AtomicBool>,
    worker: Worker<Consumer>,
}

impl Output {
    /// Opens `device` to send what `consumer`'s ring holds, which `reading`
    /// marks read, its thread woken by `signal`. A refusal hands the
    /// consumer back, except where the sending thread could not start: the
    /// failed start dropped it.
    pub(crate) fn open(
        device: &Device,
        consumer: Consumer,
        reading: Reading,
        signal: Arc<Signal>,
    ) -> Result<Output, (OpenError, Option<Consumer>)> {
        let Some(listed) = super::find(Direction::Output, device.id()) else {
            return Err((OpenError::Gone, Some(consumer)));
        };
        let sender = match Sender::open(listed.address, device) {
            Ok(sender) => sender,
            Err(why) => return Err((why, Some(consumer))),
        };
        let Some(waiter) = Waiter::new(sender.line.client.poll_fds(), signal) else {
            return Err((OpenError::NoMemory, Some(consumer)));
        };
        let left = Arc::new(AtomicBool::new(false));
        let thread_left = Arc::clone(&left);
        Worker::spawn("hapto midi out", waiter, move |waiter| {
            let _reading = reading;
            send(sender, consumer, &thread_left, waiter)
        })
        .map(|worker| Output { left, worker })
        .ok_or((OpenError::NoMemory, None))
    }

    /// Whether the device left while open.
    pub(crate) fn has_left(&self) -> bool {
        self.left.load(Ordering::Acquire)
    }

    /// Closes the device and gives its ring's consumer back, with what it
    /// had not sent dropped.
    pub(crate) fn close(mut self) -> Option<Consumer> {
        self.worker.stop()
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

/// Runs the output's thread: sends each packet once its time comes, until
/// the output ends it, then hands the ring back.
fn send(
    mut sender: Sender,
    mut consumer: Consumer,
    left: &AtomicBool,
    mut waiter: Waiter,
) -> Consumer {
    while !waiter.is_stopping() {
        let heard = match sender.step(Instant::now(), &mut consumer) {
            Next::Now => continue,
            Next::Signal => waiter.wait(None),
            Next::After(timeout) => waiter.wait(Some(timeout)),
        };
        if heard && sender.heard_left() {
            left.store(true, Ordering::Release);
        }
    }
    consumer
}

/// What the output's thread sends with, and the SysEx it is sending.
struct Sender {
    line: Line,
    /// The device's port.
    address: Address,
    sysex: OutgoingSysex,
    /// How many bytes of the joined SysEx have gone out, while some have
    /// not.
    sending: Option<usize>,
}

impl Sender {
    /// Opens a MIDI 1.0 client whose port hears the System client's
    /// announcements, then connects the port to the device at `address`,
    /// so it cannot miss the device leaving.
    fn open(address: Address, device: &Device) -> Result<Sender, OpenError> {
        let client = Client::open(Streams::Duplex, Version::Legacy)?.for_device(address);
        let caps = Caps::READ | Caps::WRITE | Caps::NO_EXPORT;
        let port = client.port(&format!("out: {}", device.name()), caps)?;
        client.connect_from(port, ANNOUNCE)?;
        client.connect_to(port, address)?;
        let parser = Parser::new().ok_or(OpenError::NoMemory)?;
        Ok(Sender {
            line: Line {
                client,
                parser,
                port,
                pace: Pace::new(),
            },
            address,
            sysex: OutgoingSysex::new(),
            sending: None,
        })
    }

    /// Whether the client heard, since the last look, that the device left.
    fn heard_left(&mut self) -> bool {
        let address = self.address;
        let mut left = false;
        let Line { client, parser, .. } = &mut self.line;
        client.read(parser, |incoming| {
            if let Incoming::Announced(change) = incoming
                && change.ends(address)
            {
                left = true;
            }
        });
        left
    }

    /// Sends the next piece of the SysEx under way, or the packet at the
    /// ring's head once it is due.
    fn step(&mut self, now: Instant, consumer: &mut Consumer) -> Next {
        if let Some(from) = self.sending {
            return self.piece(from, now, consumer);
        }
        match consumer.peek() {
            None => Next::Signal,
            Some((due, _)) if due > now => Next::After(due.since(now)),
            Some((_, words)) => self.packet(words, now, consumer),
        }
    }

    /// Sends the packet `words` at the ring's head, or joins it into the
    /// SysEx, and takes it from the ring once done with it.
    fn packet(&mut self, words: [u32; 4], now: Instant, consumer: &mut Consumer) -> Next {
        // The writer pushes only what MIDI 1.0's bytes carry.
        let Some(bytes) = Bytes::of(words) else {
            consumer.pop();
            return Next::Now;
        };
        let event = Event::new(0, words);
        if event.message_type() != MessageType::SYSEX7 {
            match self.line.send(Outgoing::Message(&bytes), now) {
                Sent::Went => {}
                Sent::Wait(wait) => return Next::After(wait),
                Sent::Refused => consumer.count_dropped(),
            }
        }
        consumer.pop();
        if self.sysex.join(&event, consumer) {
            self.sending = Some(0);
        }
        Next::Now
    }

    /// Sends the next piece of the joined SysEx, from byte `from`.
    fn piece(&mut self, from: usize, now: Instant, consumer: &Consumer) -> Next {
        let bytes = &self.sysex.bytes;
        let rest = bytes.get(from..).unwrap_or(&[]);
        let piece = rest.get(..PIECE).unwrap_or(rest);
        match self.line.send(Outgoing::Piece(piece), now) {
            Sent::Went => {
                let next = from.saturating_add(piece.len());
                self.sending = (next < bytes.len()).then_some(next);
            }
            Sent::Wait(wait) => return Next::After(wait),
            Sent::Refused => {
                consumer.count_dropped();
                self.sending = None;
            }
        }
        Next::Now
    }
}

/// The client an output sends with, and the pace it keeps.
struct Line {
    client: Client,
    parser: Parser,
    port: u8,
    pace: Pace,
}

/// What one send is.
#[derive(Clone, Copy)]
enum Outgoing<'a> {
    /// A whole MIDI 1.0 message other than a SysEx.
    Message(&'a [u8]),
    /// A piece of a SysEx.
    Piece(&'a [u8]),
}

/// What came of a send.
enum Sent {
    Went,
    /// The device has no room yet: the send waits this long.
    Wait(Duration),
    /// Refused for good.
    Refused,
}

impl Line {
    /// Sends `outgoing` once the pace allows it.
    fn send(&mut self, outgoing: Outgoing<'_>, now: Instant) -> Sent {
        let (Outgoing::Message(bytes) | Outgoing::Piece(bytes)) = outgoing;
        if let Some(wait) = self.pace.room_in(bytes.len(), now) {
            return Sent::Wait(wait);
        }
        let sent = match outgoing {
            Outgoing::Message(message) => self.client.send(&mut self.parser, self.port, message),
            Outgoing::Piece(piece) => self.client.send_sysex(self.port, piece),
        };
        match sent {
            Ok(()) => {
                self.pace.sent(bytes.len());
                Sent::Went
            }
            // The device's buffer is full after all: the pace takes the
            // port as full, and the send waits for it to drain.
            Err(SendError::Full) => {
                self.pace.fill(now);
                Sent::Wait(self.pace.room_in(bytes.len(), now).unwrap_or_default())
            }
            Err(SendError::Refused) => Sent::Refused,
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

    /// Treats the port as full at `now`.
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
    fn the_pace_keeps_a_busy_port_half_a_page_ahead_of_a_cable() {
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
