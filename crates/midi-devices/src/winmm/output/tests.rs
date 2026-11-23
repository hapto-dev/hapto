//! The output's thread's own work, with no device: a message packed as
//! WinMM takes it, and a SysEx joined from its packets.

use hapto_ump::SysExForm::{self, Complete, Continue, End, Start};

use super::*;

fn join(sysex: &mut Sysex, form: SysExForm, bytes: &[u8]) -> Joined {
    join_on(sysex, 0, form, bytes, 0)
}

/// Joins a packet on `group`, the ring's dropped count at `dropped`.
fn join_on(sysex: &mut Sysex, group: u8, form: SysExForm, bytes: &[u8], dropped: u64) -> Joined {
    let packet = SysEx7::new(group, form, bytes);
    let bytes = Bytes::of(packet.words()).expect("a SysEx packet");
    sysex.join(packet, &bytes, dropped)
}

#[test]
fn a_message_is_packed_its_status_in_the_low_byte() {
    assert_eq!(pack(&[0x90, 0x3C, 0x7F]), 0x007F_3C90);
    assert_eq!(pack(&[0xC0, 0x05]), 0x0000_05C0);
    assert_eq!(pack(&[0xF8]), 0x0000_00F8);
}

#[test]
fn a_sysex_is_joined_whole_from_its_packets() {
    let mut sysex = Sysex::new();
    assert!(matches!(
        join(&mut sysex, Complete, &[0x7E, 0x7F]),
        Joined::Whole
    ));
    assert_eq!(sysex.bytes, [0xF0, 0x7E, 0x7F, 0xF7]);
    assert!(matches!(
        join(&mut sysex, Start, &[1, 2, 3, 4, 5, 6]),
        Joined::Part
    ));
    assert!(matches!(
        join(&mut sysex, Continue, &[7, 8, 9, 10, 11, 12]),
        Joined::Part
    ));
    assert!(matches!(join(&mut sysex, End, &[13]), Joined::Whole));
    let expected: Vec<u8> = [0xF0].into_iter().chain(1..=13).chain([0xF7]).collect();
    assert_eq!(sysex.bytes, expected);
}

#[test]
fn a_sysex_begun_before_the_output_opened_or_past_its_room_is_dropped() {
    let mut sysex = Sysex::new();
    // Its start was sent before the output opened.
    assert!(matches!(
        join(&mut sysex, Continue, &[1, 2, 3]),
        Joined::Part
    ));
    assert!(matches!(join(&mut sysex, End, &[4]), Joined::Dropped));
    assert!(sysex.bytes.is_empty());
    // One longer than the room.
    assert!(matches!(join(&mut sysex, Start, &[1; 6]), Joined::Part));
    for _ in 0..SYSEX_BYTES / 6 {
        join(&mut sysex, Continue, &[2; 6]);
    }
    assert!(matches!(join(&mut sysex, End, &[3]), Joined::Dropped));
    // The next goes whole again.
    assert!(matches!(join(&mut sysex, Complete, &[9]), Joined::Whole));
    assert_eq!(sysex.bytes, [0xF0, 9, 0xF7]);
}

#[test]
fn a_sysex_that_lost_a_packet_to_a_full_ring_is_dropped() {
    let mut sysex = Sysex::new();
    assert!(matches!(
        join_on(&mut sysex, 0, Start, &[1; 6], 4),
        Joined::Part
    ));
    assert!(matches!(
        join_on(&mut sysex, 0, Continue, &[2; 6], 4),
        Joined::Part
    ));
    // The ring dropped a packet while it came.
    assert!(matches!(
        join_on(&mut sysex, 0, End, &[3], 5),
        Joined::Dropped
    ));
    // The next, with nothing dropped meanwhile, goes whole.
    assert!(matches!(
        join_on(&mut sysex, 0, Complete, &[9], 5),
        Joined::Whole
    ));
}

#[test]
fn another_groups_sysex_while_one_is_under_way_is_dropped() {
    let mut sysex = Sysex::new();
    assert!(matches!(
        join_on(&mut sysex, 0, Start, &[1; 6], 0),
        Joined::Part
    ));
    assert!(matches!(
        join_on(&mut sysex, 1, Complete, &[7], 0),
        Joined::Dropped
    ));
    // The one under way goes on, whole.
    assert!(matches!(
        join_on(&mut sysex, 0, End, &[2], 0),
        Joined::Whole
    ));
    let expected: Vec<u8> = [0xF0].into_iter().chain([1; 6]).chain([2, 0xF7]).collect();
    assert_eq!(sysex.bytes, expected);
}
