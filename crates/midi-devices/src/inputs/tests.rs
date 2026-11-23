use hapto_ump::{ChannelMessage, Origin, scale};

use super::*;

/// A song select whose song is `tag`: a system message, which the upgrade
/// passes as it is, so a test can follow it.
fn tagged(tag: u32) -> [u32; 4] {
    [0x10F3_0000 | tag << 8, 0, 0, 0]
}

fn cc(controller: u32, value: u32) -> [u32; 4] {
    [0x20B0_0000 | controller << 8 | value, 0, 0, 0]
}

fn at(millis: u64) -> Instant {
    Instant::from_nanos(millis * 1_000_000)
}

/// A reader over `MAX_INPUTS` rings of room `room`, and the producers that
/// fill them.
fn rig(room: usize) -> (Vec<Producer>, InputsReader) {
    let (producers, consumers): (Vec<_>, Vec<_>) = (0..MAX_INPUTS).map(|_| ring(room)).unzip();
    let reader = InputsReader {
        upgrades: (0..MAX_INPUTS).map(|_| Upgrade::new()).collect(),
        generations: consumers.iter().map(Consumer::generation).collect(),
        heads: vec![None; MAX_INPUTS].into_boxed_slice(),
        rings: consumers.into_boxed_slice(),
        last: None,
    };
    (producers, reader)
}

fn read(reader: &mut InputsReader, now: Instant, frames: usize) -> Vec<Event> {
    let mut out = EventList::with_capacity(32);
    reader.read(now, frames, &mut out);
    out.as_slice().to_vec()
}

/// Each tagged message read, with its frame.
fn tags(events: &[Event]) -> Vec<(u32, u32)> {
    events
        .iter()
        .map(|event| (event.offset, (event.words[0] >> 8) & 0x7F))
        .collect()
}

#[test]
fn the_time_since_the_last_block_maps_onto_this_one() {
    let (mut producers, mut reader) = rig(8);
    // The first block places everything at its first frame: it has no
    // last block to measure from.
    producers[0].push(at(1), tagged(1));
    assert_eq!(tags(&read(&mut reader, at(10), 100)), [(0, 1)]);
    // The next block spans 10 ms to 20 ms: 12.5 ms is a quarter of it.
    producers[0].push(at(12), tagged(2));
    producers[0].push(Instant::from_nanos(12_500_000), tagged(3));
    producers[0].push(at(19), tagged(4));
    let read = tags(&read(&mut reader, at(20), 100));
    assert_eq!(read, [(20, 2), (25, 3), (90, 4)]);
}

#[test]
fn a_message_that_comes_after_the_block_starts_waits_for_the_next() {
    let (mut producers, mut reader) = rig(8);
    assert_eq!(read(&mut reader, at(10), 64), []);
    producers[0].push(at(15), tagged(1));
    producers[0].push(at(25), tagged(2));
    assert_eq!(tags(&read(&mut reader, at(20), 64)), [(32, 1)]);
    // A message older than the last block lands at the first frame.
    assert_eq!(tags(&read(&mut reader, at(30), 64)), [(32, 2)]);
    producers[0].push(at(5), tagged(3));
    assert_eq!(tags(&read(&mut reader, at(40), 64)), [(0, 3)]);
}

#[test]
fn inputs_merge_in_the_order_their_messages_came_ties_by_input() {
    let (mut producers, mut reader) = rig(8);
    read(&mut reader, at(0), 10);
    producers[3].push(at(2), tagged(30));
    producers[3].push(at(6), tagged(31));
    producers[1].push(at(4), tagged(10));
    producers[0].push(at(6), tagged(0));
    let read = tags(&read(&mut reader, at(10), 10));
    assert_eq!(read, [(2, 30), (4, 10), (6, 0), (6, 31)]);
}

#[test]
fn a_block_of_no_frames_reads_nothing_and_leaves_its_messages() {
    let (mut producers, mut reader) = rig(8);
    read(&mut reader, at(0), 10);
    producers[0].push(at(5), tagged(1));
    assert_eq!(read(&mut reader, at(10), 0), []);
    assert_eq!(tags(&read(&mut reader, at(20), 10)), [(2, 1)]);
}

#[test]
fn a_message_past_the_lists_room_is_counted_there() {
    let (mut producers, mut reader) = rig(8);
    for tag in 0..5 {
        producers[0].push(at(1), tagged(tag));
    }
    let mut out = EventList::with_capacity(3);
    reader.read(at(10), 16, &mut out);
    assert_eq!((out.as_slice().len(), out.dropped()), (3, 2));
}

#[test]
fn every_input_arrives_on_group_1_upgraded_to_midi_2() {
    let (mut producers, mut reader) = rig(8);
    producers[5].push(at(1), cc(7, 127));
    let events = read(&mut reader, at(10), 10);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].origin, Origin::Midi1);
    let voice = events[0].channel_voice().expect("MIDI 2.0");
    let full = ChannelMessage::ControlChange {
        controller: 7,
        value: u32::MAX,
    };
    assert_eq!((voice.group, voice.message), (0, full));
}

#[test]
fn each_input_keeps_its_own_parameter_numbers() {
    let (mut producers, mut reader) = rig(8);
    // One device selects NRPN 300; another sends a data entry, which on its
    // own input selects nothing.
    producers[0].push(at(1), cc(99, 2));
    producers[0].push(at(1), cc(98, 44));
    producers[1].push(at(2), cc(6, 64));
    producers[0].push(at(3), cc(6, 10));
    let events = read(&mut reader, at(10), 10);
    let messages: Vec<_> = events
        .iter()
        .filter_map(|event| event.channel_voice().map(|voice| voice.message))
        .collect();
    let entry = ChannelMessage::AssignableController {
        bank: 2,
        index: 44,
        value: scale::up_14_to_32(10 << 7),
    };
    assert_eq!(messages, [entry], "the second input's entry has no address");
}

#[test]
fn a_new_device_on_an_input_starts_its_translation_over() {
    let (mut producers, mut reader) = rig(8);
    producers[0].push(at(1), cc(99, 2));
    producers[0].push(at(1), cc(98, 44));
    read(&mut reader, at(10), 10);
    // Another device takes the input, and sends a data entry alone.
    producers[0].renew();
    producers[0].push(at(12), cc(6, 10));
    assert_eq!(read(&mut reader, at(20), 10), []);
}

/// A 7-bit SysEx packet: `status` and `count` bytes of `tag`.
fn sysex(status: u32, count: u32, tag: u32) -> [u32; 4] {
    [
        0x3000_0000 | status << 20 | count << 16 | tag << 8 | tag,
        0,
        0,
        0,
    ]
}

#[test]
fn a_sysex_waits_until_whole_while_other_inputs_go_on() {
    let (mut producers, mut reader) = rig(16);
    read(&mut reader, at(0), 10);
    producers[0].push(at(1), sysex(SYSEX_START, 6, 1));
    producers[0].push(at(2), sysex(SYSEX_CONTINUE, 6, 2));
    producers[1].push(at(3), tagged(9));
    let first = read(&mut reader, at(10), 10);
    assert_eq!(tags(&first), [(3, 9)], "the other input's message goes on");
    producers[0].push(at(14), sysex(SYSEX_END, 2, 3));
    producers[1].push(at(16), tagged(8));
    let second = read(&mut reader, at(20), 10);
    // The whole SysEx, at the frame its end came, then the rest in order.
    let statuses: Vec<_> = second
        .iter()
        .map(|event| (event.offset, sysex_status(event.words)))
        .collect();
    assert_eq!(
        statuses,
        [
            (4, Some(SYSEX_START)),
            (4, Some(SYSEX_CONTINUE)),
            (4, Some(SYSEX_END)),
            (6, None)
        ]
    );
}

#[test]
fn a_real_time_message_inside_a_sysex_goes_with_it() {
    let (mut producers, mut reader) = rig(16);
    let clock = [0x10F8_0000, 0, 0, 0];
    producers[0].push(at(1), sysex(SYSEX_START, 6, 1));
    producers[0].push(at(2), clock);
    producers[0].push(at(3), sysex(SYSEX_END, 1, 2));
    let words: Vec<u32> = read(&mut reader, at(10), 10)
        .iter()
        .map(|event| event.words[0])
        .collect();
    assert_eq!(
        words,
        [
            sysex(SYSEX_START, 6, 1)[0],
            clock[0],
            sysex(SYSEX_END, 1, 2)[0]
        ]
    );
}

#[test]
fn a_sysex_longer_than_the_room_goes_out_as_it_comes() {
    // Room for 4 packets, all of them the SysEx so far.
    let (mut producers, mut reader) = rig(4);
    producers[0].push(at(1), sysex(SYSEX_START, 6, 1));
    for tag in 2..5 {
        producers[0].push(at(u64::from(tag)), sysex(SYSEX_CONTINUE, 6, tag));
    }
    assert_eq!(read(&mut reader, at(10), 10).len(), 4);
}

#[test]
fn a_held_sysex_goes_out_while_a_devices_next_buffer_still_fits() {
    // Room for 512 packets: a SysEx held for its end goes out as it comes
    // once fewer than a burst's packets of room are left, past 340 held.
    let held = |count: u32| {
        let (mut producers, mut reader) = rig(512);
        producers[0].push(at(1), sysex(SYSEX_START, 6, 1));
        for tag in 2..=count {
            producers[0].push(at(2), sysex(SYSEX_CONTINUE, 6, tag));
        }
        let mut out = EventList::with_capacity(512);
        reader.read(at(10), 10, &mut out);
        out.as_slice().len()
    };
    assert_eq!(held(300), 0, "room for a burst: it waits for its end");
    assert_eq!(
        held(360),
        360,
        "a burst might not fit: it goes out as it came"
    );
}

#[test]
fn a_sysex_the_device_dropped_part_way_goes_as_it_is() {
    let (mut producers, mut reader) = rig(8);
    producers[0].push(at(1), sysex(SYSEX_START, 6, 1));
    producers[0].push(at(2), tagged(5));
    let statuses: Vec<_> = read(&mut reader, at(10), 10)
        .iter()
        .map(|event| sysex_status(event.words))
        .collect();
    assert_eq!(statuses, [Some(SYSEX_START), None]);
}

#[test]
fn an_output_is_refused_before_any_input_is_taken() {
    let (mut inputs, _reader) = inputs(8);
    let id = DeviceId::new(Direction::Output, "Synth", 0);
    let synth = Device::new(id, "Synth".into(), Direction::Output);
    assert_eq!(inputs.open(&synth), Err(OpenError::NotAnInput));
    assert_eq!(inputs.devices().count(), 0);
}

#[test]
fn a_device_not_in_the_list_is_gone_and_leaves_its_input_free() {
    let (mut inputs, _reader) = inputs(8);
    let id = DeviceId::new(Direction::Input, "no such device", 0);
    let keys = Device::new(id, "Keys".into(), Direction::Input);
    let refused = inputs.open(&keys);
    assert!(
        matches!(refused, Err(OpenError::Gone | OpenError::NoBackend)),
        "{refused:?}"
    );
    assert!(
        inputs
            .slots
            .iter()
            .all(|slot| matches!(slot, Slot::Free(_)))
    );
}
