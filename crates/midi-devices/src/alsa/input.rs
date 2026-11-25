//! An open input: a sequencer client of the app's own subscribed to the
//! device's port, and the thread that reads what the device sends into the
//! input's ring.
//!
//! A MIDI 1.0 device's events become bytes, read with `ByteStream` as
//! WinMM's are, and the reader upgrades them by Hapto's own rules. A
//! MIDI 2.0 device's packets go into the ring as they came, on their own
//! groups.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use hapto_ump::{ByteStream, MessageType, SysEx7, SysExForm};

use super::Listed;
use super::seq::{ANNOUNCE, Address, Caps, Client, Incoming, Parser, Streams, Version};
use super::signal::Signal;
use super::worker::{Waiter, Worker};
use crate::Instant;
use crate::error::OpenError;
use crate::id::{Device, Direction};
use crate::ring::Producer;

/// An input, reading into its ring while open. It closes as it drops.
#[derive(Debug)]
pub(crate) struct Input {
    /// Set once the device's port is gone.
    left: Arc<AtomicBool>,
    worker: Worker<Producer>,
}

impl Input {
    /// Opens `device` to read into `producer`'s ring. A refusal hands the
    /// producer back, except where the reading thread could not start: the
    /// failed start dropped it.
    pub(crate) fn open(
        device: &Device,
        producer: Producer,
    ) -> Result<Input, (OpenError, Option<Producer>)> {
        let Some(listed) = super::find(Direction::Input, device.id()) else {
            return Err((OpenError::Gone, Some(producer)));
        };
        let (client, parser) = match open_client(&listed, device) {
            Ok(opened) => opened,
            Err(why) => return Err((why, Some(producer))),
        };
        let Some(waiter) = Waiter::new(client.poll_fds(), Arc::new(Signal::new())) else {
            return Err((OpenError::NoMemory, Some(producer)));
        };
        let left = Arc::new(AtomicBool::new(false));
        let thread_left = Arc::clone(&left);
        let (address, reading) = (listed.address, Reading::new(producer));
        Worker::spawn("hapto midi in", waiter, move |waiter| {
            read(client, parser, address, reading, &thread_left, waiter)
        })
        .map(|worker| Input { left, worker })
        .ok_or((OpenError::NoMemory, None))
    }

    /// Whether the device left while open.
    pub(crate) fn has_left(&self) -> bool {
        self.left.load(Ordering::Acquire)
    }

    /// Closes the device and gives its ring's producer back.
    pub(crate) fn close(mut self) -> Option<Producer> {
        self.worker.stop()
    }
}

/// Opens a client for `device` at the device's own MIDI version, or, where
/// the kernel refuses that version, at MIDI 1.0, which the kernel
/// translates for. The client's port hears the System client's
/// announcements before the device's port, so it cannot miss the device
/// leaving.
fn open_client(listed: &Listed, device: &Device) -> Result<(Client, Parser), OpenError> {
    let client = Client::open(Streams::Input, listed.version)
        .or_else(|_| Client::open(Streams::Input, Version::Legacy))?
        .with_input_room()
        .for_device(listed.address);
    let port = client.port(
        &format!("in: {}", device.name()),
        Caps::WRITE | Caps::NO_EXPORT,
    )?;
    client.connect_from(port, ANNOUNCE)?;
    client.connect_from(port, listed.address)?;
    let parser = Parser::new().ok_or(OpenError::NoMemory)?;
    Ok((client, parser))
}

/// Runs the input's thread: reads what the device at `address` sends into
/// the ring, stamped as it comes, until the input ends it, then hands the
/// producer back.
fn read(
    mut client: Client,
    mut parser: Parser,
    address: Address,
    mut reading: Reading,
    left: &AtomicBool,
    mut waiter: Waiter,
) -> Producer {
    while !waiter.is_stopping() {
        if !waiter.wait(None) {
            continue;
        }
        let at = Instant::now();
        client.read(&mut parser, |incoming| match incoming {
            Incoming::Packet(words) => reading.packet(at, words),
            Incoming::Bytes(bytes) => reading.bytes(at, bytes),
            Incoming::Announced(change) if change.ends(address) => {
                left.store(true, Ordering::Release);
            }
            Incoming::Overrun => reading.cut(at),
            Incoming::Announced(_) | Incoming::Other => {}
        });
    }
    reading.into_producer()
}

/// What an input reads into: its ring, with a MIDI 1.0 device's byte
/// stream, read onto group 1 as a host merges devices, or where each of a
/// MIDI 2.0 device's groups stands in a SysEx.
struct Reading {
    producer: Producer,
    stream: ByteStream,
    under_way: UnderWay,
}

impl Reading {
    fn new(producer: Producer) -> Reading {
        Reading {
            producer,
            stream: ByteStream::new(0),
            under_way: UnderWay::default(),
        }
    }

    fn bytes(&mut self, at: Instant, bytes: &[u8]) {
        let producer = &mut self.producer;
        self.stream.read(bytes, |words| producer.push(at, words));
    }

    fn packet(&mut self, at: Instant, words: [u32; 4]) {
        if self.under_way.passes(words) {
            self.producer.push(at, trimmed(words));
        }
    }

    /// Ends any SysEx under way with the part that came, and passes over
    /// the rest of it: after an overrun, so the reader never holds a SysEx
    /// with a hole, as after a WinMM driver's error, and at the close, so it
    /// never holds one waiting for an end that never comes.
    fn cut(&mut self, at: Instant) {
        let producer = &mut self.producer;
        self.stream.cut(|words| producer.push(at, words));
        self.under_way.cut(|words| producer.push(at, words));
    }

    /// The ring's producer, any SysEx under way cut.
    fn into_producer(mut self) -> Producer {
        self.cut(Instant::now());
        self.producer
    }
}

/// The packet `words` hold, with the words past its length zeroed.
fn trimmed(mut words: [u32; 4]) -> [u32; 4] {
    let length = MessageType::of(words[0]).words();
    for word in words.iter_mut().skip(length) {
        *word = 0;
    }
    words
}

/// Where each group of a MIDI 2.0 device stands in a SysEx, so a cut can
/// end each one under way, as `ByteStream::cut` ends a MIDI 1.0 device's.
#[derive(Debug, Default)]
struct UnderWay([Stand; 16]);

/// Where one group stands in a SysEx.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Stand {
    /// No SysEx is under way.
    #[default]
    Idle,
    /// A SysEx began and has not ended.
    Begun,
    /// A SysEx was cut here: what comes of the rest of it is passed over.
    Cut,
}

impl UnderWay {
    /// Whether `words`, a packet the device sent, goes on to the ring: not
    /// if it is the rest of a SysEx cut here.
    fn passes(&mut self, words: [u32; 4]) -> bool {
        let Some(packet) = SysEx7::decode(words) else {
            return true;
        };
        let Some(stand) = self.0.get_mut(usize::from(packet.group)) else {
            return true;
        };
        let passes = packet.form.begins() || *stand != Stand::Cut;
        if packet.form.ends() {
            *stand = Stand::Idle;
        } else if packet.form.begins() {
            *stand = Stand::Begun;
        }
        passes
    }

    /// Ends every SysEx under way, handing `send` an end of no bytes for
    /// each.
    fn cut(&mut self, mut send: impl FnMut([u32; 4])) {
        for (group, stand) in (0_u8..).zip(&mut self.0) {
            if *stand == Stand::Begun {
                send(SysEx7::new(group, SysExForm::End, &[]).words());
                *stand = Stand::Cut;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_ends_a_midi_2_sysex_and_passes_over_the_rest_of_it() {
        let packet = |group, form, data: &[u8]| SysEx7::new(group, form, data).words();
        let mut under_way = UnderWay::default();
        assert!(under_way.passes(packet(3, SysExForm::Start, &[1; 6])));
        // A note passes in between, even on the SysEx's group.
        assert!(under_way.passes([0x4390_3C00, 0xC800_0000, 0, 0]));
        let mut ended = Vec::new();
        under_way.cut(|words| ended.push(words));
        assert_eq!(ended, [packet(3, SysExForm::End, &[])]);
        // The rest of the cut SysEx is passed over, and the next passes.
        assert!(!under_way.passes(packet(3, SysExForm::Continue, &[2; 6])));
        assert!(!under_way.passes(packet(3, SysExForm::End, &[3])));
        assert!(under_way.passes(packet(3, SysExForm::Complete, &[4])));
        // With nothing under way, a cut ends nothing.
        under_way.cut(|words| ended.push(words));
        assert_eq!(ended.len(), 1);
    }
}
