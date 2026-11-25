//! Tests a message split into six-byte SysEx7 packets and read back.

use hapto_ump::{Event, MessageType, SysEx7, SysExForm};

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
