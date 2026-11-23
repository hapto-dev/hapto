//! The output's thread's own work, with no device: a message packed as
//! WinMM takes it, and a SysEx joined from its packets.

use hapto_ump::SysEx7;
use hapto_ump::SysExForm::{self, Complete, Continue, End, Start};

use super::*;

/// Joins a SysEx packet on group 0, the ring having dropped nothing.
fn join(sysex: &mut Sysex, form: SysExForm, bytes: &[u8]) -> Joined {
    join_on(sysex, 0, form, bytes, 0)
}

/// Joins a SysEx packet on `group`, the ring's dropped count at `dropped`.
fn join_on(sysex: &mut Sysex, group: u8, form: SysExForm, bytes: &[u8], dropped: u64) -> Joined {
    sysex.join(&SysEx7::new(group, form, bytes).event(0), dropped)
}

const PART: Joined = Joined {
    whole: false,
    dropped: 0,
};
const WHOLE: Joined = Joined {
    whole: true,
    dropped: 0,
};
const DROPPED: Joined = Joined {
    whole: false,
    dropped: 1,
};

#[test]
fn a_message_is_packed_its_status_in_the_low_byte() {
    assert_eq!(pack(&[0x90, 0x3C, 0x7F]), 0x007F_3C90);
    assert_eq!(pack(&[0xC0, 0x05]), 0x0000_05C0);
    assert_eq!(pack(&[0xF8]), 0x0000_00F8);
}

#[test]
fn a_sysex_is_joined_whole_from_its_packets() {
    let mut sysex = Sysex::new();
    assert_eq!(join(&mut sysex, Complete, &[0x7E, 0x7F]), WHOLE);
    assert_eq!(sysex.bytes, [0xF0, 0x7E, 0x7F, 0xF7]);
    assert_eq!(join(&mut sysex, Start, &[1, 2, 3, 4, 5, 6]), PART);
    assert_eq!(join(&mut sysex, Continue, &[7, 8, 9, 10, 11, 12]), PART);
    assert_eq!(join(&mut sysex, End, &[13]), WHOLE);
    let expected: Vec<u8> = [0xF0].into_iter().chain(1..=13).chain([0xF7]).collect();
    assert_eq!(sysex.bytes, expected);
}

#[test]
fn a_sysex_begun_before_the_output_opened_or_past_its_room_is_dropped() {
    let mut sysex = Sysex::new();
    // Its start was sent before the output opened.
    assert_eq!(join(&mut sysex, Continue, &[1, 2, 3]), PART);
    assert_eq!(join(&mut sysex, End, &[4]), DROPPED);
    // One longer than the room.
    assert_eq!(join(&mut sysex, Start, &[1; 6]), PART);
    for _ in 0..SYSEX_BYTES / 6 {
        join(&mut sysex, Continue, &[2; 6]);
    }
    assert_eq!(join(&mut sysex, End, &[3]), DROPPED);
    // The next goes whole again.
    assert_eq!(join(&mut sysex, Complete, &[9]), WHOLE);
    assert_eq!(sysex.bytes, [0xF0, 9, 0xF7]);
}

#[test]
fn a_sysex_that_lost_a_packet_to_a_full_ring_is_dropped() {
    let mut sysex = Sysex::new();
    assert_eq!(join_on(&mut sysex, 0, Start, &[1; 6], 4), PART);
    assert_eq!(join_on(&mut sysex, 0, Continue, &[2; 6], 4), PART);
    // The ring dropped a packet while it came.
    assert_eq!(join_on(&mut sysex, 0, End, &[3], 5), DROPPED);
    // The next, with nothing dropped meanwhile but the one counted, goes
    // whole.
    assert_eq!(join_on(&mut sysex, 0, Complete, &[9], 6), WHOLE);
}

#[test]
fn another_groups_sysex_while_one_is_under_way_is_dropped_and_the_first_goes_on() {
    let mut sysex = Sysex::new();
    assert_eq!(join_on(&mut sysex, 0, Start, &[1; 6], 0), PART);
    assert_eq!(join_on(&mut sysex, 1, Complete, &[7], 0), DROPPED);
    // The one under way goes on whole: the drop counted on the ring was
    // this output's own, not a packet lost.
    assert_eq!(join_on(&mut sysex, 0, End, &[2], 1), WHOLE);
    let expected: Vec<u8> = [0xF0].into_iter().chain([1; 6]).chain([2, 0xF7]).collect();
    assert_eq!(sysex.bytes, expected);
}

#[test]
fn a_message_on_its_group_ends_a_sysex_under_way() {
    let mut sysex = Sysex::new();
    assert_eq!(join(&mut sysex, Start, &[1; 6]), PART);
    assert_eq!(
        sysex.join(&Event::new(0, [0x2090_3C7F, 0, 0, 0]), 0),
        DROPPED
    );
    // The rest of it passes unread, and the next goes whole.
    assert_eq!(join_on(&mut sysex, 0, End, &[2], 1), PART);
    assert_eq!(join_on(&mut sysex, 0, Complete, &[9], 1), WHOLE);
}
