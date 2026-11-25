//! Tests the backend against the sequencer itself, with clients of the
//! tests' own whose ports any app may subscribe to, as a keyboard's or a
//! synth's are.
//!
//! They need the sequencer, `/dev/snd/seq`, but no hardware. The USB card
//! test reads a sysfs tree that it writes itself.

use hapto_testkit::sync::wait_until;
use hapto_ump::{Event, EventList, SysEx7, SysExJoin};

use super::seq::{Caps, Client, Incoming, Parser, Streams, Version};
use crate::{Device, Devices, Direction, InputsReader, Instant, inputs, outputs};

/// A test's own device, a client with one port that any app may subscribe
/// to, which leaves when it drops.
struct Fake {
    client: Client,
    parser: Parser,
    port: u8,
}

impl Fake {
    fn open(name: &str, caps: Caps, version: Version) -> Option<Fake> {
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

    /// Opens a keyboard, whose port sends.
    fn keys(name: &str) -> Fake {
        Fake::open(name, Caps::READ | Caps::SUBS_READ, Version::Legacy).expect("a MIDI 1.0 client")
    }

    /// Opens a synth, whose port receives.
    fn synth(name: &str) -> Fake {
        Fake::open(name, Caps::WRITE | Caps::SUBS_WRITE, Version::Legacy)
            .expect("a MIDI 1.0 client")
    }

    /// Opens a MIDI 2.0 keyboard, if alsa-lib and the kernel speak UMP.
    fn midi2_keys(name: &str) -> Option<Fake> {
        Fake::open(name, Caps::READ | Caps::SUBS_READ, Version::UmpMidi2)
    }

    /// Sends `bytes`, splitting a SysEx into pieces as a card's port does.
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

    /// Returns the bytes that the port took, once `done` accepts them.
    fn heard(&mut self, done: impl Fn(&[u8]) -> bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        wait_until("the test's synth to hear it", || {
            bytes.extend(self.taken());
            done(&bytes)
        });
        bytes
    }

    /// Returns the bytes that the port took since the last call.
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

/// Waits until the list holds the device called `name` in `direction`,
/// and returns it.
fn listed(name: &str, direction: Direction) -> Device {
    let mut found = None;
    wait_until(&format!("{name} to be listed"), || {
        found = super::list()
            .into_iter()
            .find(|device| device.name() == name && device.direction() == direction);
        found.is_some()
    });
    found.expect("listed")
}

/// Returns the MIDI read into blocks of 512 frames, once `done` accepts it.
fn read_until(reader: &mut InputsReader, done: impl Fn(&[Event]) -> bool) -> Vec<Event> {
    let mut read = Vec::new();
    wait_until("the input to read it", || {
        let mut block = EventList::with_capacity(4096);
        reader.read(Instant::now(), 512, &mut block);
        read.extend_from_slice(block.as_slice());
        done(&read)
    });
    read
}

/// Returns a framed SysEx of `length` data bytes.
fn sysex(length: usize) -> Vec<u8> {
    let data = (0..length).map(|place| (place % 0x80) as u8);
    [0xF0].into_iter().chain(data).chain([0xF7]).collect()
}

/// Returns the framed SysEx that `events` join into.
fn joined(events: &[Event]) -> Option<Vec<u8>> {
    let mut join = SysExJoin::with_room(64 * 1024);
    events
        .iter()
        .find_map(|event| join.push(event).whole.map(|whole| whole.framed().to_vec()))
}

/// Returns the SysEx7 packets on group 1 that carry `framed`.
fn packets(framed: &[u8]) -> Vec<Event> {
    let data = framed.get(1..framed.len().saturating_sub(1)).unwrap_or(&[]);
    let mut events = Vec::new();
    SysEx7::packets(0, data, |words| events.push(Event::new(0, words)));
    events
}

#[test]
fn a_subscribable_port_is_listed_in_each_direction_it_allows() {
    let _keys = Fake::keys("listed keys");
    let _synth = Fake::synth("listed synth");
    let keys = listed("listed keys", Direction::Input);
    assert!(keys.id().as_str().starts_with("in:"));
    assert!(keys.id().as_str().contains("listed keys"));
    listed("listed synth", Direction::Output);
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
        .open(&listed("hidden keys", Direction::Input))
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
        .open(&listed("upgraded keys", Direction::Input))
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
        .open(&listed("sysex keys", Direction::Input))
        .expect("the test's keys open");
    let sent = sysex(6000);
    keys.play(&sent);
    let read = read_until(&mut reader, |read| joined(read).is_some());
    assert_eq!(joined(&read), Some(sent));
}

#[test]
fn a_sysex_sent_as_one_event_reaches_the_block_whole() {
    // As RtMidi and midir send one, however long.
    let mut keys = Fake::keys("one event keys");
    let (mut inputs, mut reader) = inputs(8192);
    inputs
        .open(&listed("one event keys", Direction::Input))
        .expect("the test's keys open");
    let sent = sysex(20_000);
    keys.client
        .send_sysex(keys.port, &sent)
        .expect("the sequencer takes it");
    let read = read_until(&mut reader, |read| joined(read).is_some());
    assert_eq!(joined(&read), Some(sent));
}

#[test]
fn only_its_device_reaches_an_input() {
    let mut keys = Fake::keys("guarded keys");
    let (mut inputs, mut reader) = inputs(1024);
    inputs
        .open(&listed("guarded keys", Direction::Input))
        .expect("the test's keys open");
    let mut other = Fake::keys("other keys");
    // The input's own port, which no list holds.
    let input = other
        .client
        .ports()
        .into_iter()
        .find(|entry| entry.name == "in: guarded keys")
        .expect("the input's port")
        .address;
    assert!(
        other.client.connect_to(other.port, input).is_err(),
        "no other app subscribes to it"
    );
    other
        .client
        .send_to(&mut other.parser, other.port, input, &[0x90, 0x40, 100])
        .expect("the sequencer takes it");
    keys.play(&[0x90, 0x3C, 100]);
    let key_60 = |event: &Event| event.words[0] == 0x4090_3C00;
    let read = read_until(&mut reader, |read| read.iter().any(key_60));
    assert!(read.iter().all(key_60), "{read:?}");
}

#[test]
fn a_midi_2_devices_packets_reach_the_block_unchanged_on_their_groups() {
    let Some(mut keys) = Fake::midi2_keys("midi 2 keys") else {
        eprintln!("skipped: this alsa-lib or kernel has no UMP");
        return;
    };
    let (mut inputs, mut reader) = inputs(1024);
    inputs
        .open(&listed("midi 2 keys", Direction::Input))
        .expect("the test's keys open");
    // MIDI 2.0 note-ons on group 4, keys 60 to 62, velocity 0xC800, sent at
    // once so the input reads them in one wake.
    let notes: Vec<[u32; 4]> = (0..3)
        .map(|key| [0x4390_3C00 + (key << 8), 0xC800_0000, 0, 0])
        .collect();
    for &words in &notes {
        keys.client
            .send_packet(keys.port, words)
            .expect("the sequencer takes it");
    }
    let read = read_until(&mut reader, |read| read.len() >= notes.len());
    assert_eq!(
        read.iter().map(|event| event.words).collect::<Vec<_>>(),
        notes
    );
}

#[test]
fn a_device_that_leaves_is_closed_at_the_tick() {
    let keys = Fake::keys("leaving keys");
    let device = listed("leaving keys", Direction::Input);
    let (mut inputs, _reader) = inputs(64);
    inputs.open(&device).expect("the test's keys open");
    drop(keys);
    let mut left = Vec::new();
    wait_until("a tick to close the device that left", || {
        left = inputs.tick();
        !left.is_empty()
    });
    assert_eq!(left, std::slice::from_ref(&device));
    assert!(!inputs.is_open(device.id()));
}

#[test]
fn a_device_that_comes_is_added_at_a_tick() {
    let mut devices = Devices::new();
    assert!(devices.is_watching());
    let _keys = Fake::keys("coming keys");
    wait_until("a tick to add the device that came", || {
        devices
            .tick(std::time::Instant::now())
            .added
            .iter()
            .any(|device| device.name() == "coming keys")
    });
}

#[test]
fn an_output_sends_a_long_sysex_whole_and_its_close_sends_nothing() {
    let mut synth = Fake::synth("played synth");
    let device = listed("played synth", Direction::Output);
    let (mut outputs, mut writer) = outputs(1024);
    outputs.open(&device).expect("the test's synth opens");
    let long = sysex(1000);
    let mut sent = vec![Event::new(0, [0x2090_3C64, 0, 0, 0])];
    sent.extend(packets(&long));
    writer.write(Instant::now(), 48_000.0, &sent);
    let expected: Vec<u8> = [0x90, 0x3C, 0x64].into_iter().chain(long).collect();
    let heard = synth.heard(|heard| heard.len() >= expected.len());
    assert_eq!(heard, expected);
    // The close joins the thread, and the sequencer passes a direct event on
    // as it is sent, so the synth already has anything that the close sent.
    assert!(outputs.close(device.id()));
    assert_eq!(synth.taken(), Vec::<u8>::new(), "a close sends nothing");
}

#[test]
fn a_message_on_a_sysexs_group_drops_the_sysex_as_on_windows() {
    let mut synth = Fake::synth("cut synth");
    let device = listed("cut synth", Direction::Output);
    let (mut outputs, mut writer) = outputs(1024);
    outputs.open(&device).expect("the test's synth opens");
    // A SysEx's start, a note on its group, then its end.
    let mut sent = packets(&sysex(12));
    sent.insert(1, Event::new(0, [0x2090_3C64, 0, 0, 0]));
    writer.write(Instant::now(), 48_000.0, &sent);
    assert_eq!(synth.heard(|heard| heard.len() >= 3), [0x90, 0x3C, 0x64]);
    wait_until("the SysEx to be dropped", || writer.dropped() > 0);
    assert_eq!(writer.dropped(), 1, "the SysEx, dropped and counted");
    assert_eq!(synth.taken(), Vec::<u8>::new(), "none of it went out");
}

#[test]
fn a_usb_card_is_keyed_by_its_serial_only_where_it_has_one() {
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
