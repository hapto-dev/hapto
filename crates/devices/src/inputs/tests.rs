use super::*;

fn note(tag: u32) -> [u32; 4] {
    [0x2090_3C00 | tag, 0, 0, 0]
}

fn at(millis: u64) -> Instant {
    Instant::from_nanos(millis * 1_000_000)
}

/// A reader over `GROUPS` rings, and the producers that fill them.
fn rig() -> (Vec<Producer>, InputsReader) {
    let (producers, consumers): (Vec<_>, Vec<_>) = (0..GROUPS).map(|_| ring(8)).unzip();
    let reader = InputsReader {
        rings: consumers.into_boxed_slice(),
        last: None,
    };
    (producers, reader)
}

fn read(reader: &mut InputsReader, now: Instant, frames: usize) -> Vec<(u32, u32)> {
    let mut out = EventList::with_capacity(32);
    reader.read(now, frames, &mut out);
    out.as_slice()
        .iter()
        .map(|event| (event.offset, event.words[0] & 0x7F))
        .collect()
}

#[test]
fn the_time_since_the_last_block_maps_onto_this_one() {
    let (mut producers, mut reader) = rig();
    // The first block places everything at its first frame: it has no
    // last block to measure from.
    producers[0].push(at(1), note(1));
    assert_eq!(read(&mut reader, at(10), 100), [(0, 1)]);
    // The next block spans 10 ms to 20 ms: 12.5 ms is a quarter of it.
    producers[0].push(at(12), note(2));
    producers[0].push(Instant::from_nanos(12_500_000), note(3));
    producers[0].push(at(19), note(4));
    assert_eq!(read(&mut reader, at(20), 100), [(20, 2), (25, 3), (90, 4)]);
}

#[test]
fn a_message_that_comes_after_the_block_starts_waits_for_the_next() {
    let (mut producers, mut reader) = rig();
    assert_eq!(read(&mut reader, at(10), 64), []);
    producers[0].push(at(15), note(1));
    producers[0].push(at(25), note(2));
    assert_eq!(read(&mut reader, at(20), 64), [(32, 1)]);
    // A message older than the last block lands at the first frame.
    assert_eq!(read(&mut reader, at(30), 64), [(32, 2)]);
    producers[0].push(at(5), note(3));
    assert_eq!(read(&mut reader, at(40), 64), [(0, 3)]);
}

#[test]
fn inputs_merge_in_the_order_their_messages_came_ties_by_group() {
    let (mut producers, mut reader) = rig();
    read(&mut reader, at(0), 10);
    producers[3].push(at(2), note(30));
    producers[3].push(at(6), note(31));
    producers[1].push(at(4), note(10));
    producers[0].push(at(6), note(0));
    let read = read(&mut reader, at(10), 10);
    assert_eq!(read, [(2, 30), (4, 10), (6, 0), (6, 31)]);
}

#[test]
fn a_block_of_no_frames_reads_nothing_and_leaves_its_messages() {
    let (mut producers, mut reader) = rig();
    read(&mut reader, at(0), 10);
    producers[0].push(at(5), note(1));
    assert_eq!(read(&mut reader, at(10), 0), []);
    assert_eq!(read(&mut reader, at(20), 10), [(2, 1)]);
}

#[test]
fn a_message_past_the_lists_room_is_counted_there() {
    let (mut producers, mut reader) = rig();
    for tag in 0..5 {
        producers[0].push(at(1), note(tag));
    }
    let mut out = EventList::with_capacity(3);
    reader.read(at(10), 16, &mut out);
    assert_eq!((out.as_slice().len(), out.dropped()), (3, 2));
}

#[test]
fn an_output_is_refused_before_any_group_is_taken() {
    let (mut inputs, _reader) = inputs(8);
    let synth = Device::new(
        DeviceId::new(Direction::Output, "Synth", 0),
        "Synth".into(),
        Direction::Output,
    );
    assert_eq!(inputs.open(&synth), Err(OpenError::NotAnInput));
    assert_eq!(inputs.devices().count(), 0);
}

#[test]
fn a_device_not_in_the_list_is_gone_and_leaves_its_group_free() {
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
