//! The output's thread's own work, with no device: a message packed as
//! WinMM takes it, and a SysEx joined from its packets.

use super::*;

fn sysex_packet(status: u32, bytes: &[u8]) -> [u32; 4] {
    let mut payload = [0_u32; 6];
    for (slot, &byte) in payload.iter_mut().zip(bytes) {
        *slot = u32::from(byte);
    }
    let count = bytes.len() as u32;
    [
        0x3000_0000 | status << 20 | count << 16 | payload[0] << 8 | payload[1],
        payload[2] << 24 | payload[3] << 16 | payload[4] << 8 | payload[5],
        0,
        0,
    ]
}

fn join(sysex: &mut Sysex, status: u32, bytes: &[u8]) -> Joined {
    let words = sysex_packet(status, bytes);
    let bytes = Bytes::of(words).expect("a SysEx packet");
    sysex.join(words, &bytes)
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
    assert!(matches!(join(&mut sysex, 0, &[0x7E, 0x7F]), Joined::Whole));
    assert_eq!(sysex.bytes, [0xF0, 0x7E, 0x7F, 0xF7]);
    assert!(matches!(
        join(&mut sysex, 1, &[1, 2, 3, 4, 5, 6]),
        Joined::Part
    ));
    assert!(matches!(
        join(&mut sysex, 2, &[7, 8, 9, 10, 11, 12]),
        Joined::Part
    ));
    assert!(matches!(join(&mut sysex, 3, &[13]), Joined::Whole));
    let expected: Vec<u8> = [0xF0].into_iter().chain(1..=13).chain([0xF7]).collect();
    assert_eq!(sysex.bytes, expected);
}

#[test]
fn a_sysex_begun_before_the_output_opened_or_past_its_room_is_dropped() {
    let mut sysex = Sysex::new();
    // Its start was sent before the output opened.
    assert!(matches!(join(&mut sysex, 2, &[1, 2, 3]), Joined::Part));
    assert!(matches!(join(&mut sysex, 3, &[4]), Joined::Dropped));
    assert!(sysex.bytes.is_empty());
    // One longer than the room.
    assert!(matches!(join(&mut sysex, 1, &[1; 6]), Joined::Part));
    for _ in 0..SYSEX_BYTES / 6 {
        join(&mut sysex, 2, &[2; 6]);
    }
    assert!(matches!(join(&mut sysex, 3, &[3]), Joined::Dropped));
    // The next goes whole again.
    assert!(matches!(join(&mut sysex, 0, &[9]), Joined::Whole));
    assert_eq!(sysex.bytes, [0xF0, 9, 0xF7]);
}
