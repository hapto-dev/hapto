//! Tests a message split into six-byte SysEx7 packets and read back.

use hapto_ump::{Event, MessageType, SysEx7, SysExForm, SysExJoin};

#[test]
fn a_message_of_every_length_splits_into_its_packets_and_reads_back() {
    for len in 0..=40_usize {
        let data: Vec<u8> = (0..len).map(|index| (index * 37) as u8).collect();
        let mut packets = Vec::new();
        SysEx7::packets(5, &data, |words| packets.push(words));
        assert_eq!(packets.len(), SysEx7::count(len), "{len} bytes");
        let read: Vec<SysEx7> = packets
            .iter()
            .map(|&words| SysEx7::decode(words).expect("a SysEx7 packet"))
            .collect();
        let forms: Vec<SysExForm> = read.iter().map(|packet| packet.form).collect();
        if len <= SysEx7::MAX_DATA {
            assert_eq!(forms, [SysExForm::Complete], "{len} bytes");
        } else {
            assert_eq!(forms.first(), Some(&SysExForm::Start), "{len} bytes");
            assert_eq!(forms.last(), Some(&SysExForm::End), "{len} bytes");
            let middle = &forms[1..forms.len() - 1];
            assert!(middle.iter().all(|&form| form == SysExForm::Continue));
        }
        assert!(read.iter().all(|packet| packet.group == 5));
        let joined: Vec<u8> = read
            .iter()
            .flat_map(|packet| packet.data().to_vec())
            .collect();
        let sent: Vec<u8> = data.iter().map(|byte| byte & 0x7F).collect();
        assert_eq!(joined, sent, "{len} bytes, each held to 7 bits");
    }
}

#[test]
fn only_a_sysex7_packet_with_a_defined_status_decodes() {
    // A MIDI 1.0 note-on is not a SysEx message.
    assert_eq!(SysEx7::decode([0x2090_3C7F, 0, 0, 0]), None);
    // Statuses 4 to 15 are reserved.
    for status in 4..16_u32 {
        assert_eq!(SysEx7::decode([0x3002_0102 | status << 20, 0, 0, 0]), None);
    }
    // A count above six is read as six, and the group is still read.
    let packet = SysEx7::decode([0x3B3F_0102, 0x0304_0506, 0, 0]).expect("whole");
    assert_eq!((packet.group, packet.form), (0xB, SysExForm::End));
    assert_eq!(packet.data(), [1, 2, 3, 4, 5, 6]);
    // Bytes after the count are not the packet's data.
    let short = SysEx7::decode([0x3002_0102, 0x0304_0506, 0, 0]).expect("whole");
    assert_eq!(short, SysEx7::new(0, SysExForm::Complete, &[1, 2]));
}

#[test]
fn a_packet_holds_six_bytes_of_seven_bits_on_its_group() {
    let packet = SysEx7::new(0x1F, SysExForm::Start, &[0xFF, 1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(packet.data(), [0x7F, 1, 2, 3, 4, 5]);
    let event = packet.event(9);
    assert_eq!(event.offset, 9);
    assert_eq!(event.message_type(), MessageType::SYSEX7);
    assert_eq!((packet.group, event.group()), (0xF, 0xF));
    assert_eq!(event.sysex7(), Some(packet));
    assert_eq!(Event::new(0, [0x10F8_0000, 0, 0, 0]).sysex7(), None);
}

/// Returns `data`'s packets on `group`, as events.
fn message(group: u8, data: &[u8]) -> Vec<Event> {
    let mut packets = Vec::new();
    SysEx7::packets(group, data, |words| packets.push(Event::new(0, words)));
    packets
}

/// Returns, for each packet, the data that it made whole and how many
/// messages it dropped.
fn join_all(join: &mut SysExJoin, packets: &[Event]) -> Vec<(Option<Vec<u8>>, u32)> {
    packets
        .iter()
        .map(|packet| {
            let joined = join.push(packet);
            (
                joined.whole.map(|whole| whole.data().to_vec()),
                joined.dropped,
            )
        })
        .collect()
}

#[test]
fn a_sysex_comes_whole_at_its_last_packet_and_lasts_until_the_next() {
    let mut join = SysExJoin::with_room(64);
    let data: Vec<u8> = (0..20).collect();
    let packets = message(3, &data);
    for packet in &packets[..3] {
        assert_eq!(join.push(packet).whole, None);
    }
    let whole = join.push(&packets[3]).whole.expect("whole");
    assert_eq!((whole.group, whole.data()), (3, &data[..]));
    let framed: Vec<u8> = [0xF0]
        .into_iter()
        .chain(data.clone())
        .chain([0xF7])
        .collect();
    assert_eq!(whole.framed(), framed);
    assert!(join.message().is_some(), "until the next packet");
    join.push(&Event::new(0, [0x10F8_0000, 0, 0, 0]));
    assert_eq!(join.message(), None);
}

#[test]
fn another_groups_sysex_under_way_is_dropped_at_its_start() {
    let mut join = SysExJoin::with_room(64);
    let first = message(0, &[1; 8]);
    let second = message(1, &[2; 8]);
    let sent = [first[0], second[0], second[1], first[1]];
    assert_eq!(
        join_all(&mut join, &sent),
        [(None, 0), (None, 1), (None, 0), (Some(vec![1; 8]), 0)]
    );
    // The rest of the dropped SysEx is skipped, even after the first one
    // ends.
    let sent = [first[0], second[0], first[1], second[1]];
    assert_eq!(
        join_all(&mut join, &sent),
        [(None, 0), (None, 1), (Some(vec![1; 8]), 0), (None, 0)]
    );
}

#[test]
fn any_other_message_on_its_group_ends_a_sysex_but_real_time_and_groupless_ones() {
    let mut join = SysExJoin::with_room(64);
    let packets = message(2, &[5; 8]);
    let clock = Event::new(0, [0x12F8_0000, 0, 0, 0]);
    let utility = Event::new(0, [0x0000_0000, 0, 0, 0]);
    let elsewhere = Event::new(0, [0x2390_3C7F, 0, 0, 0]);
    let sent = [packets[0], clock, utility, elsewhere, packets[1]];
    assert_eq!(
        join_all(&mut join, &sent).last(),
        Some(&(Some(vec![5; 8]), 0))
    );
    // A note-on, and a packet with a reserved status, on its own group.
    for ender in [[0x2290_3C7F, 0, 0, 0], [0x3242_0101, 0, 0, 0]] {
        let sent = [packets[0], Event::new(0, ender), packets[1]];
        assert_eq!(
            join_all(&mut join, &sent),
            [(None, 0), (None, 1), (None, 0)],
            "ended, and the rest of it passes unread"
        );
    }
}

#[test]
fn a_new_start_on_its_group_ends_the_sysex_under_way() {
    let mut join = SysExJoin::with_room(64);
    let old = message(0, &[1; 8]);
    let new = message(0, &[2; 3]);
    assert_eq!(
        join_all(&mut join, &[old[0], new[0]]),
        [(None, 0), (Some(vec![2; 3]), 1)]
    );
}

#[test]
fn a_part_with_no_start_or_past_the_room_is_dropped_at_its_end() {
    let mut join = SysExJoin::with_room(12);
    let long = message(0, &[1; 20]);
    assert_eq!(
        join_all(&mut join, &long[1..]),
        [(None, 0), (None, 0), (None, 1)]
    );
    assert_eq!(
        join_all(&mut join, &long).last(),
        Some(&(None, 1)),
        "past 12 bytes"
    );
    let fits = message(0, &[3; 12]);
    assert_eq!(
        join_all(&mut join, &fits).last(),
        Some(&(Some(vec![3; 12]), 0))
    );
    join.push(&long[0]);
    join.reset();
    assert_eq!(join_all(&mut join, &long[3..]), [(None, 1)], "forgotten");
}
