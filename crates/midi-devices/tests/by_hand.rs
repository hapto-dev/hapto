//! The computer's own MIDI devices, by hand: each test is ignored, since it
//! needs a device and a person. `cargo test -p hapto-midi-devices --test
//! by_hand -- --ignored --nocapture --test-threads 1`, with a test's name
//! after `--ignored` for that one alone.
//!
//! The tests that replug or unplug a device follow the one whose name holds
//! `HAPTO_DEVICE`, else the first that is not the system's synth. The long
//! SysEx in needs a loopback port, such as loopMIDI's, whose name starts
//! with `HAPTO_LOOPBACK`, else with `loopMIDI`.

use std::thread;
use std::time::Duration;

use hapto_midi_devices::{
    Changes, Device, Devices, Direction, Inputs, Instant, MAX_INPUTS, MAX_OUTPUTS, Outputs, Timing,
    inputs, outputs,
};
use hapto_ump::{Event, EventList, SysEx7, SysExJoin};

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

/// The device the person chose by `HAPTO_DEVICE`, a part of its name, among
/// `listed`, else the first that is not the system's synth.
fn chosen<'a>(mut listed: impl Iterator<Item = &'a Device>) -> Option<&'a Device> {
    match std::env::var("HAPTO_DEVICE") {
        Ok(part) => listed.find(|device| device.name().contains(&part)),
        Err(_) => listed.find(|device| device.name() != SYNTH),
    }
}

/// `length` bytes of SysEx for no one's gear, 0x7D being the id MIDI keeps
/// for non-commercial use, and its packets as events at frame 0.
fn long_sysex(length: usize) -> (Vec<u8>, Vec<Event>) {
    let data: Vec<u8> = std::iter::once(0x7D)
        .chain((1..length).map(|index| (index % 0x7F) as u8))
        .collect();
    let mut events = Vec::new();
    SysEx7::packets(0, &data, |words| events.push(Event::new(0, words)));
    (data, events)
}

/// Ticks the list as an app's main thread does, about 60 times a second,
/// handing `each` what came and went, until it says it is done.
///
/// # Panics
///
/// After `limit`, naming what it was `waiting_for`.
fn tick_until(
    devices: &mut Devices,
    limit: Duration,
    waiting_for: &str,
    mut each: impl FnMut(&Changes) -> bool,
) {
    let start = std::time::Instant::now();
    loop {
        if each(&devices.tick(std::time::Instant::now())) {
            return;
        }
        assert!(start.elapsed() < limit, "no {waiting_for} within {limit:?}");
        thread::sleep(Duration::from_millis(16));
    }
}

/// Opens every listed direction of the device called `name`.
fn open_both(devices: &Devices, name: &str, inputs: &mut Inputs, outputs: &mut Outputs) {
    for device in devices.all().iter().filter(|device| device.name() == name) {
        let opened = match device.direction() {
            Direction::Input => inputs.open(device),
            Direction::Output => outputs.open(device),
        };
        assert_eq!(opened, Ok(()), "{name} opens");
    }
}

#[test]
#[ignore = "by hand: unplug and plug in a USB MIDI device 17 times, then play a few keys"]
fn a_device_replugged_past_the_slots_opens_every_time_and_still_reads() {
    let mut devices = Devices::new();
    let Some(name) = chosen(devices.inputs()).map(|device| device.name().to_owned()) else {
        println!("no input to replug here");
        return;
    };
    let (mut inputs, mut reader) = inputs(1024);
    let (mut outputs, _writer) = outputs(64);
    open_both(&devices, &name, &mut inputs, &mut outputs);
    // One past every slot, so each replug takes a slot a device left.
    let replugs = MAX_INPUTS.max(MAX_OUTPUTS) + 1;
    for round in 1..=replugs {
        println!("{round} of {replugs}: unplug {name}");
        tick_until(&mut devices, Duration::from_secs(60), "unplug", |changes| {
            for device in &changes.removed {
                inputs.close(device.id());
                outputs.close(device.id());
            }
            // A device found gone as it was read closes here too.
            inputs.tick();
            outputs.tick();
            changes.removed.iter().any(|device| device.name() == name)
        });
        assert_eq!(inputs.devices().count(), 0, "nothing open once it went");
        println!("  and plug it back in");
        tick_until(
            &mut devices,
            Duration::from_secs(60),
            "plug-in",
            |changes| changes.added.iter().any(|device| device.name() == name),
        );
        open_both(&devices, &name, &mut inputs, &mut outputs);
    }
    println!("play a few keys on {name} now");
    let mut heard = 0;
    for _ in 0..50 {
        thread::sleep(Duration::from_millis(100));
        let mut block = EventList::with_capacity(256);
        reader.read(Instant::now(), 4800, &mut block);
        heard += block.as_slice().len();
    }
    assert!(
        heard > 0,
        "the keys reached a block after {replugs} replugs"
    );
    assert_eq!(reader.dropped(), 0);
}

#[test]
#[ignore = "by hand: needs a loopback port, which brings a long SysEx sent to it back in"]
fn a_sysex_longer_than_the_inputs_room_comes_in_whole() {
    let devices = Devices::new();
    let port = std::env::var("HAPTO_LOOPBACK").unwrap_or_else(|_| "loopMIDI".to_owned());
    let named = |device: &&Device| device.name().starts_with(&port);
    let (Some(out), Some(back)) = (devices.outputs().find(named), devices.inputs().find(named))
    else {
        println!("no loopback port named {port}* here: add one in loopMIDI");
        return;
    };
    // Room for 1024 packets is 6 KB of SysEx, and the SysEx is five times
    // that, so it comes in a part at a time as the room fills.
    let (mut inputs, mut reader) = inputs(1024);
    let (mut outputs, mut writer) = outputs(16384);
    inputs.open(back).expect("the loopback's input opens");
    outputs.open(out).expect("the loopback's output opens");
    let (sent, events) = long_sysex(32 * 1024);
    writer.write(Instant::now(), 48_000.0, &events);
    // Blocks of 10 ms, as an audio thread reads them, for up to 10 s.
    let mut join = SysExJoin::with_room(64 * 1024);
    let mut came = None;
    for _ in 0..1000 {
        thread::sleep(Duration::from_millis(10));
        let mut block = EventList::with_capacity(8192);
        reader.read(Instant::now(), 480, &mut block);
        for event in block.as_slice() {
            let joined = join.push(event);
            assert_eq!(joined.dropped, 0, "no part of it dropped");
            if let Some(whole) = joined.whole {
                came = Some(whole.data().to_vec());
            }
        }
        if came.is_some() {
            break;
        }
    }
    let came = came.expect("the SysEx came back within ten seconds");
    assert_eq!(came.len(), sent.len());
    assert!(came == sent, "byte for byte");
    assert_eq!(
        (reader.dropped(), writer.dropped(), writer.lost()),
        (0, 0, 0)
    );
}

/// A note-on and its note-off on each of the first eight keys from middle C,
/// at frame 0.
fn notes() -> Vec<Event> {
    (60..68)
        .flat_map(|key: u32| {
            [
                Event::new(0, [0x4090_0000 | key << 8, 0x8000_0000, 0, 0]),
                Event::new(0, [0x4080_0000 | key << 8, 0, 0, 0]),
            ]
        })
        .collect()
}

/// Notes, not SysEx: through Windows 10's WDM driver, a device unplugged
/// while a long SysEx goes out to it leaves the send stuck in the driver,
/// and the process cannot end after it, as the open note on `Outputs` says.
/// Short messages are safe, so they are what goes out as the device leaves.
#[test]
#[ignore = "by hand: unplug a USB MIDI device while notes go out to it, then plug it in"]
fn a_device_unplugged_while_notes_go_out_closes_at_once_and_opens_again() {
    let mut devices = Devices::new();
    let Some(device) = chosen(devices.outputs()).cloned() else {
        println!("no output to unplug here");
        return;
    };
    let name = device.name().to_owned();
    let (mut outputs, mut writer) = outputs(16384);
    outputs.open(&device).expect("it opens");
    let notes = notes();
    println!("unplug {name} within twenty seconds: notes go out to it meanwhile");
    let mut closed_in = None;
    tick_until(&mut devices, Duration::from_secs(20), "unplug", |changes| {
        writer.write(Instant::now(), 48_000.0, &notes);
        let closing = std::time::Instant::now();
        let left = !outputs.tick().is_empty();
        let removed = changes.removed.iter().any(|gone| gone.id() == device.id());
        if removed {
            outputs.close(device.id());
        }
        if left || removed {
            closed_in = Some(closing.elapsed());
        }
        left || removed
    });
    let closed_in = closed_in.expect("it closed");
    println!("closed in {closed_in:?}");
    assert!(closed_in < Duration::from_secs(2), "a close does not wait");
    assert_eq!(outputs.devices().count(), 0);
    println!("now plug {name} back in");
    tick_until(
        &mut devices,
        Duration::from_secs(60),
        "plug-in",
        |changes| changes.added.iter().any(|came| came.name() == name),
    );
    let again = devices
        .outputs()
        .find(|came| came.name() == name)
        .expect("listed again");
    assert_eq!(outputs.open(again), Ok(()), "it opens again");
    let dropped = writer.dropped();
    writer.write(Instant::now(), 48_000.0, &notes);
    thread::sleep(Duration::from_millis(500));
    assert_eq!(writer.dropped(), dropped, "notes go out again");
    assert!(outputs.close(again.id()));
}
