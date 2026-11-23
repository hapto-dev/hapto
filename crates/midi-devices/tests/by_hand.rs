//! The computer's own MIDI devices, by hand: each test is ignored, since it
//! needs a device and a person. `cargo test -p hapto-midi-devices --test
//! by_hand -- --ignored --nocapture --test-threads 1`.

use std::thread;
use std::time::Duration;

use hapto_midi_devices::{Devices, Instant, Timing, inputs, outputs};
use hapto_ump::{Event, EventList};

/// The system's own synth, which plays what it is sent through the
/// speakers.
const SYNTH: &str = "Microsoft GS Wavetable Synth";

#[test]
#[ignore = "by hand: lists the computer's MIDI devices"]
fn every_device_the_system_lists_shows_its_name_and_direction() {
    for device in Devices::new().all() {
        println!(
            "{:?}: {} ({})",
            device.direction(),
            device.name(),
            device.id().as_str()
        );
    }
}

#[test]
#[ignore = "by hand: plays a chord on the system's synth, through the speakers"]
fn a_chord_and_a_sysex_play_on_the_systems_synth_each_at_its_time() {
    let devices = Devices::new();
    let Some(synth) = devices.outputs().find(|device| device.name() == SYNTH) else {
        println!("no {SYNTH} here");
        return;
    };
    let (mut outputs, mut writer) = outputs(64);
    outputs.set_timing(Timing::AtFrames);
    outputs.open(synth).expect("the synth opens");
    // General MIDI on, then C, E and G a quarter second apart, held to a
    // second, as one block of a second at 1 kHz.
    let on = |frame: u32, key: u32| Event::new(frame, [0x4090_0000 | key << 8, 0xC000_0000, 0, 0]);
    let off = |frame: u32, key: u32| Event::new(frame, [0x4080_0000 | key << 8, 0, 0, 0]);
    let block = [
        Event::new(0, [0x3004_7E7F, 0x0901_0000, 0, 0]),
        on(0, 60),
        on(250, 64),
        on(500, 67),
        off(1000, 60),
        off(1000, 64),
        off(1000, 67),
    ];
    writer.write(Instant::now(), 1_000.0, &block);
    thread::sleep(Duration::from_millis(1300));
    assert_eq!((writer.dropped(), writer.lost()), (0, 0));
    assert!(outputs.close(synth.id()));
}

#[test]
#[ignore = "by hand: opens every output and tells it to stop every note"]
fn every_output_opens_takes_all_notes_off_and_closes() {
    let devices = Devices::new();
    let (mut outputs, mut writer) = outputs(64);
    for device in devices.outputs() {
        match outputs.open(device) {
            Ok(()) => println!("sending to {}", device.name()),
            Err(why) => println!("{}: {why}", device.name()),
        }
    }
    // Controller 123, all notes off, on the first channel.
    writer.write(
        Instant::now(),
        48_000.0,
        &[Event::new(0, [0x40B0_7B00, 0, 0, 0])],
    );
    thread::sleep(Duration::from_millis(50));
    assert_eq!((writer.dropped(), writer.lost()), (0, 0));
    let open: Vec<_> = outputs
        .devices()
        .map(|device| device.id().clone())
        .collect();
    for id in &open {
        assert!(outputs.close(id));
    }
    assert_eq!(outputs.devices().count(), 0);
}

#[test]
#[ignore = "by hand: play keys on a keyboard within five seconds"]
fn what_is_played_on_every_input_reaches_a_block() {
    let devices = Devices::new();
    let (mut inputs, mut reader) = inputs(1024);
    for device in devices.inputs() {
        match inputs.open(device) {
            Ok(()) => println!("reading {}", device.name()),
            Err(why) => println!("{}: {why}", device.name()),
        }
    }
    println!("play a few keys now");
    let mut heard = Vec::new();
    for _ in 0..50 {
        thread::sleep(Duration::from_millis(100));
        let mut block = EventList::with_capacity(256);
        reader.read(Instant::now(), 4800, &mut block);
        heard.extend(block.as_slice().iter().map(|event| event.words[0]));
    }
    for words in &heard {
        println!("{words:08X}");
    }
    assert_eq!(reader.dropped(), 0);
}
