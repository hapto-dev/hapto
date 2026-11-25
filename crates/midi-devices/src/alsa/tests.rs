//! The backend against the sequencer itself, with devices of the tests' own:
//! clients whose ports any app may subscribe to, as a keyboard's or a
//! synth's are. They need the sequencer, `/dev/snd/seq`, and no hardware.
//! A USB card's name is read from files of a test's own.

use std::thread;
use std::time::Duration;

use hapto_ump::{Event, EventList, SysEx7, SysExForm, SysExJoin};

use super::seq::{CAP_READ, CAP_SUBS_READ, CAP_SUBS_WRITE, CAP_WRITE, Client, Incoming, Parser};
use super::seq::{Streams, Version};
use crate::{Device, Devices, Direction, InputsReader, Instant, inputs, outputs};

/// How long a test waits for the sequencer to hand something on.
const PATIENCE: Duration = Duration::from_secs(5);

/// A device of a test's own: a client with one port any app may subscribe
/// to. It leaves as it drops.
struct Fake {
    client: Client,
    parser: Parser,
    port: u8,
}

impl Fake {
    fn open(name: &str, caps: u32, version: Version) -> Option<Fake> {
        let client = match Client::open(Streams::Duplex, version) {
            Ok(client) => client,
            Err(why) if version == Version::Legacy => {
                panic!("these tests need the ALSA sequencer, /dev/snd/seq: {why}")
            }
            Err(_) => return None,
        };
        let port = client.port(name, caps).expect("a port of the test's own");
        Some(Fake {
            client,
            parser: Parser::new().expect("a parser"),
            port,
        })
    }

    /// A keyboard: a port that sends.
    fn keys(name: &str) -> Fake {
        Fake::open(name, CAP_READ | CAP_SUBS_READ, Version::Legacy).expect("a MIDI 1.0 client")
    }

    /// A synth: a port that takes.
    fn synth(name: &str) -> Fake {
        Fake::open(name, CAP_WRITE | CAP_SUBS_WRITE, Version::Legacy).expect("a MIDI 1.0 client")
    }

    /// A MIDI 2.0 keyboard, where alsa-lib and the kernel speak UMP.
    fn midi2_keys(name: &str) -> Option<Fake> {
        Fake::open(name, CAP_READ | CAP_SUBS_READ, Version::UmpMidi2)
    }

    /// Sends `bytes`, a SysEx in pieces as a card's port hands one on.
    fn play(&mut self, bytes: &[u8]) {
        if bytes.first() == Some(&0xF0) {
            for piece in bytes.chunks(256) {
                self.client
                    .send_sysex(self.port, piece)
                    .expect("the sequencer takes it");
            }
        } else {
            self.client
                .send(&mut self.parser, self.port, bytes)
                .expect("the sequencer takes it");
        }
    }

    /// The bytes the port took, until `done` says they are enough.
    fn heard(&mut self, done: impl Fn(&[u8]) -> bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        let start = std::time::Instant::now();
        while !done(&bytes) && start.elapsed() < PATIENCE {
            bytes.extend(self.taken());
            thread::sleep(Duration::from_millis(5));
        }
        bytes
    }

    /// The bytes the port took since the last look.
    fn taken(&mut self) -> Vec<u8> {
        let mut bytes = Vec::new();
        self.client.read(&mut self.parser, |incoming| {
            if let Incoming::Bytes(heard) = incoming {
                bytes.extend_from_slice(heard);
            }
        });
        bytes
    }
}

/// The device the list names `name` in its direction, once it lists it.
fn listed(name: &str, outputs: bool) -> Device {
    let direction = if outputs {
        Direction::Output
    } else {
        Direction::Input
    };
    let start = std::time::Instant::now();
    loop {
        let found = super::list()
            .into_iter()
            .find(|device| device.name() == name && device.direction() == direction);
        if let Some(device) = found {
            return device;
        }
        assert!(start.elapsed() < PATIENCE, "{name} is never listed");
        thread::sleep(Duration::from_millis(5));
    }
}

/// The MIDI read into blocks of 512 frames until `done` says it is enough.
fn read_until(reader: &mut InputsReader, done: impl Fn(&[Event]) -> bool) -> Vec<Event> {
    let mut read = Vec::new();
    let start = std::time::Instant::now();
    while !done(&read) && start.elapsed() < PATIENCE {
        let mut block = EventList::with_capacity(4096);
        reader.read(Instant::now(), 512, &mut block);
        read.extend_from_slice(block.as_slice());
        thread::sleep(Duration::from_millis(5));
    }
    read
}

/// A SysEx of `length` data bytes, framed.
fn sysex(length: usize) -> Vec<u8> {
    let data = (0..length).map(|place| (place % 0x80) as u8);
    [0xF0].into_iter().chain(data).chain([0xF7]).collect()
}

/// The SysEx `events` join to, framed.
fn joined(events: &[Event]) -> Option<Vec<u8>> {
    let mut join = SysExJoin::with_room(64 * 1024);
    events
        .iter()
        .find_map(|event| join.push(event).whole.map(|whole| whole.framed().to_vec()))
}

#[test]
fn a_port_any_app_may_subscribe_to_is_listed_in_each_direction_it_allows() {
    let _keys = Fake::keys("listed keys");
    let _synth = Fake::synth("listed synth");
    let keys = listed("listed keys", false);
    assert!(keys.id().as_str().starts_with("in:"));
    assert!(keys.id().as_str().contains("listed keys"));
    listed("listed synth", true);
    let devices = Devices::new();
    assert!(
        devices
            .outputs()
            .all(|device| device.name() != "listed keys")
    );
    assert!(
        devices
            .inputs()
            .all(|device| device.name() != "listed synth")
    );
}

#[test]
fn the_apps_own_ports_are_never_listed() {
    let _keys = Fake::keys("hidden keys");
    let (mut inputs, _reader) = inputs(64);
    inputs
        .open(&listed("hidden keys", false))
        .expect("the test's keys open");
    let devices = Devices::new();
    assert!(
        devices
            .all()
            .iter()
            .all(|device| !device.name().starts_with("in: ")),
        "{:?}",
        devices.all()
    );
}

#[test]
fn what_a_midi_1_device_plays_reaches_the_block_upgraded_on_group_1() {
    let mut keys = Fake::keys("upgraded keys");
    let (mut inputs, mut reader) = inputs(1024);
    inputs
        .open(&listed("upgraded keys", false))
        .expect("the test's keys open");
    keys.play(&[0x90, 0x3C, 100]);
    let read = read_until(&mut reader, |read| !read.is_empty());
    let [note] = read.as_slice() else {
        panic!("one note: {read:?}")
    };
    // A MIDI 2.0 note-on on group 1, channel 1, key 60, attribute none.
    assert_eq!(note.words[0], 0x4090_3C00);
    // Velocity 100 at 16 bits, by the Default Translation.
    assert_eq!(note.words[1] >> 16, 0xC924);
}

#[test]
fn a_sysex_longer_than_a_page_reaches_the_block_whole() {
    let mut keys = Fake::keys("sysex keys");
    let (mut inputs, mut reader) = inputs(4096);
    inputs
        .open(&listed("sysex keys", false))
        .expect("the test's keys open");
    let sent = sysex(6000);
    keys.play(&sent);
    let read = read_until(&mut reader, |read| joined(read).is_some());
    assert_eq!(joined(&read), Some(sent));
}

#[test]
fn a_midi_2_device_is_read_as_it_sends_on_its_own_group() {
    let Some(mut keys) = Fake::midi2_keys("midi 2 keys") else {
        eprintln!("skipped: this alsa-lib or kernel has no UMP");
        return;
    };
    let (mut inputs, mut reader) = inputs(1024);
    inputs
        .open(&listed("midi 2 keys", false))
        .expect("the test's keys open");
    // A MIDI 2.0 note-on on group 4, key 60, velocity 0xC800.
    let words = [0x4390_3C00, 0xC800_0000, 0, 0];
    keys.client
        .send_packet(keys.port, words)
        .expect("the sequencer takes it");
    let read = read_until(&mut reader, |read| !read.is_empty());
    assert_eq!(
        read.iter().map(|event| event.words).collect::<Vec<_>>(),
        [words]
    );
}

#[test]
fn a_device_that_leaves_is_closed_at_the_tick() {
    let keys = Fake::keys("leaving keys");
    let device = listed("leaving keys", false);
    let (mut inputs, _reader) = inputs(64);
    inputs.open(&device).expect("the test's keys open");
    drop(keys);
    let start = std::time::Instant::now();
    let left = loop {
        let left = inputs.tick();
        if !left.is_empty() || start.elapsed() > PATIENCE {
            break left;
        }
        thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(left, std::slice::from_ref(&device));
    assert!(!inputs.is_open(device.id()));
}

#[test]
fn a_device_that_comes_is_told_at_a_tick_after_the_list_is_marked() {
    let mut devices = Devices::new();
    assert!(devices.is_watching());
    let _keys = Fake::keys("coming keys");
    let start = std::time::Instant::now();
    let came = loop {
        let changes = devices.tick(std::time::Instant::now());
        if let Some(device) = changes
            .added
            .into_iter()
            .find(|device| device.name() == "coming keys")
        {
            break Some(device);
        }
        if start.elapsed() > PATIENCE {
            break None;
        }
        thread::sleep(Duration::from_millis(20));
    };
    assert!(came.is_some(), "told within a few looks");
}

#[test]
fn an_output_sends_a_note_and_a_long_sysex_whole_and_nothing_as_it_closes() {
    let mut synth = Fake::synth("played synth");
    let device = listed("played synth", true);
    let (mut outputs, mut writer) = outputs(1024);
    outputs.open(&device).expect("the test's synth opens");
    let long = sysex(1000);
    let mut sent = vec![Event::new(0, [0x2090_3C64, 0, 0, 0])];
    sent.extend(packets(&long));
    writer.write(Instant::now(), 48_000.0, &sent);
    let expected: Vec<u8> = [0x90, 0x3C, 0x64].into_iter().chain(long).collect();
    let heard = synth.heard(|heard| heard.len() >= expected.len());
    assert_eq!(heard, expected);
    // The close joins the thread, and the sequencer hands a direct event on
    // as it is sent, so whatever the close sent the synth has by now.
    assert!(outputs.close(device.id()));
    assert_eq!(synth.taken(), Vec::<u8>::new(), "a close sends nothing");
}

#[test]
fn a_usb_card_with_a_serial_is_named_by_it_and_one_without_by_where_it_sits() {
    let usb = std::env::temp_dir()
        .join("hapto-midi-devices-tests")
        .join(format!("usb-{}", std::process::id()))
        .join("1-2");
    let _ = std::fs::remove_dir_all(&usb);
    let interface = usb.join("1-2:1.0");
    std::fs::create_dir_all(&interface).expect("made");
    std::fs::write(usb.join("idVendor"), "258f\n").expect("written");
    std::fs::write(usb.join("idProduct"), "0b01\n").expect("written");
    assert_eq!(super::usb_key(&interface), None, "no serial");
    std::fs::write(usb.join("serial"), "\n").expect("written");
    assert_eq!(super::usb_key(&interface), None, "an empty serial");
    std::fs::write(usb.join("serial"), "109330573537\n").expect("written");
    assert_eq!(
        super::usb_key(&interface).as_deref(),
        Some("usb:258f:0b01:109330573537:1.0")
    );
    let _ = std::fs::remove_dir_all(&usb);
}

/// The SysEx7 packets that carry `framed`.
fn packets(framed: &[u8]) -> Vec<Event> {
    let data = framed.get(1..framed.len().saturating_sub(1)).unwrap_or(&[]);
    let chunks: Vec<&[u8]> = data.chunks(6).collect();
    chunks
        .iter()
        .enumerate()
        .map(|(place, chunk)| {
            let form = match (place, chunks.len()) {
                (_, 1) => SysExForm::Complete,
                (0, _) => SysExForm::Start,
                (place, count) if place + 1 == count => SysExForm::End,
                _ => SysExForm::Continue,
            };
            SysEx7::new(0, form, chunk).event(0)
        })
        .collect()
}
