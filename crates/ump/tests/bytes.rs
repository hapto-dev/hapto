//! MIDI 1.0's byte stream read into packets and written back, as a device
//! that speaks it meets Hapto's UMP.

use hapto_ump::{ByteStream, Bytes};
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

fn read(bytes: &[u8]) -> Vec<[u32; 4]> {
    let mut packets = Vec::new();
    ByteStream::new(0).read(bytes, |words| packets.push(words));
    packets
}

fn note_on(note: u32, velocity: u32) -> [u32; 4] {
    [0x2090_0000 | note << 8 | velocity, 0, 0, 0]
}

const CLOCK: [u32; 4] = [0x10F8_0000, 0, 0, 0];

#[test]
fn running_status_carries_a_channel_status_to_the_data_after_it() {
    let sent = [0x90, 0x3C, 0x7F, 0x3E, 0x7F, 0x3C, 0x00];
    assert_eq!(
        read(&sent),
        [note_on(0x3C, 0x7F), note_on(0x3E, 0x7F), note_on(0x3C, 0)]
    );
    // A program change wants one data byte.
    assert_eq!(
        read(&[0xC5, 0x01, 0x02]),
        [[0x20C5_0100, 0, 0, 0], [0x20C5_0200, 0, 0, 0]]
    );
}

#[test]
fn a_real_time_byte_goes_out_at_once_and_leaves_the_message_it_falls_in() {
    assert_eq!(
        read(&[0x90, 0x3C, 0xF8, 0x7F]),
        [CLOCK, note_on(0x3C, 0x7F)]
    );
    // Inside a SysEx too, which it leaves whole.
    let sent = [0xF0, 0x7E, 0xF8, 0x7F, 0x09, 0x01, 0xF7];
    assert_eq!(read(&sent), [CLOCK, [0x3004_7E7F, 0x0901_0000, 0, 0]]);
    // It does not end running status.
    assert_eq!(
        read(&[0x90, 0x3C, 0x7F, 0xFE, 0x3E, 0x7F]),
        [
            note_on(0x3C, 0x7F),
            [0x10FE_0000, 0, 0, 0],
            note_on(0x3E, 0x7F)
        ]
    );
}

#[test]
fn system_common_messages_carry_their_data_and_end_running_status() {
    let sent = [
        0x90, 0x3C, 0x7F, 0xF3, 0x05, 0x3C, 0x7F, 0xF2, 0x10, 0x20, 0xF1, 0x31, 0xF6,
    ];
    assert_eq!(
        read(&sent),
        [
            note_on(0x3C, 0x7F),
            // Song select, then the note's bytes dropped: no status holds.
            [0x10F3_0500, 0, 0, 0],
            // Song position, its low 7 bits first, as MIDI 1.0 sends it.
            [0x10F2_1020, 0, 0, 0],
            [0x10F1_3100, 0, 0, 0],
            [0x10F6_0000, 0, 0, 0],
        ]
    );
}

#[test]
fn what_has_no_status_or_no_meaning_is_dropped() {
    // Data before any status, a stray end of SysEx, and the undefined
    // statuses, which end running status.
    let sent = [
        0x3C, 0x7F, 0xF7, 0x40, 0xF4, 0x01, 0xF5, 0xF9, 0xFD, 0x90, 0xF4, 0x3C, 0x7F,
    ];
    assert_eq!(read(&sent), Vec::<[u32; 4]>::new());
}

#[test]
fn a_status_that_cuts_a_sysex_short_ends_it_with_what_it_holds() {
    let sent = [0xF0, 0x01, 0x02, 0x90, 0x3C, 0x7F];
    assert_eq!(read(&sent), [[0x3002_0102, 0, 0, 0], note_on(0x3C, 0x7F)]);
    // A new SysEx ends the old one and begins.
    let sent = [0xF0, 0x01, 0xF0, 0x02, 0xF7];
    assert_eq!(
        read(&sent),
        [[0x3001_0100, 0, 0, 0], [0x3001_0200, 0, 0, 0]]
    );
}

#[test]
fn the_group_goes_on_every_packet() {
    let mut packets = Vec::new();
    ByteStream::new(5).read(&[0x90, 0x3C, 0x7F, 0xF8, 0xF0, 0xF7], |words| {
        packets.push(words)
    });
    assert_eq!(
        packets,
        [
            [0x2590_3C7F, 0, 0, 0],
            [0x15F8_0000, 0, 0, 0],
            [0x3500_0000, 0, 0, 0]
        ]
    );
}

/// The packets a SysEx of `data` is, built from the specification's rule
/// rather than the stream's: one Complete packet when it fits, otherwise a
/// Start, Continues and an End of six bytes each but the last.
fn sysex_packets(data: &[u8]) -> Vec<[u32; 4]> {
    let chunks: Vec<&[u8]> = if data.is_empty() {
        vec![&[]]
    } else {
        data.chunks(6).collect()
    };
    let last = chunks.len() - 1;
    chunks
        .iter()
        .enumerate()
        .map(|(index, chunk)| {
            let status: u32 = match (index, last) {
                (0, 0) => 0,
                (0, _) => 1,
                (i, l) if i == l => 3,
                _ => 2,
            };
            let mut bytes = [0_u32; 6];
            for (slot, &byte) in bytes.iter_mut().zip(*chunk) {
                *slot = u32::from(byte);
            }
            let count = chunk.len() as u32;
            [
                0x3000_0000 | status << 20 | count << 16 | bytes[0] << 8 | bytes[1],
                bytes[2] << 24 | bytes[3] << 16 | bytes[4] << 8 | bytes[5],
                0,
                0,
            ]
        })
        .collect()
}

#[test]
fn a_sysex_of_every_length_goes_out_six_bytes_at_a_time_and_writes_back() {
    for length in 0..=20_u8 {
        let data: Vec<u8> = (1..=length).collect();
        let mut sent = vec![0xF0];
        sent.extend(&data);
        sent.push(0xF7);
        let packets = read(&sent);
        assert_eq!(packets, sysex_packets(&data), "{length} bytes");
        let back: Vec<u8> = packets
            .iter()
            .flat_map(|&words| Bytes::of(words).expect("SysEx7").to_vec())
            .collect();
        assert_eq!(back, sent, "{length} bytes");
    }
}

#[test]
fn known_packets_write_as_their_bytes() {
    let cases: [([u32; 4], &[u8]); 9] = [
        ([0x2090_3C7F, 0, 0, 0], &[0x90, 0x3C, 0x7F]),
        ([0x20C5_0100, 0, 0, 0], &[0xC5, 0x01]),
        ([0x20D0_4000, 0, 0, 0], &[0xD0, 0x40]),
        ([0x20E0_0040, 0, 0, 0], &[0xE0, 0x00, 0x40]),
        ([0x10F2_1020, 0, 0, 0], &[0xF2, 0x10, 0x20]),
        ([0x10F3_0500, 0, 0, 0], &[0xF3, 0x05]),
        ([0x10F8_0000, 0, 0, 0], &[0xF8]),
        ([0x3002_0102, 0, 0, 0], &[0xF0, 0x01, 0x02, 0xF7]),
        (
            [0x3126_0102, 0x0304_0506, 0, 0],
            &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06],
        ),
    ];
    for (words, bytes) in cases {
        assert_eq!(Bytes::of(words).as_deref(), Some(bytes), "{words:08X?}");
    }
    // Nothing MIDI 1.0's byte stream has: MIDI 2.0, a utility message,
    // 8-bit SysEx, an undefined system status, a SysEx status past End.
    for words in [
        [0x4090_3C00, 0xFFFF_0000, 0, 0],
        [0x0000_0000, 0, 0, 0],
        [0x5001_0000, 0, 0, 0],
        [0x10F4_0000, 0, 0, 0],
        [0x3040_0000, 0, 0, 0],
    ] {
        assert_eq!(Bytes::of(words), None, "{words:08X?}");
    }
}

/// A MIDI 1.0 message for the stream to read.
#[derive(Debug, Clone)]
enum Sent {
    Channel { status: u8, first: u8, second: u8 },
    Common { status: u8, first: u8, second: u8 },
    SysEx(Vec<u8>),
}

impl Sent {
    /// The message's bytes, its status left out when `running` already
    /// holds it, and the packets it is.
    fn bytes_and_packets(&self, running: &mut Option<u8>) -> (Vec<u8>, Vec<[u32; 4]>) {
        match self {
            Sent::Channel {
                status,
                first,
                second,
            } => {
                let two = !matches!(status >> 4, 0xC | 0xD);
                let mut bytes = Vec::new();
                if *running != Some(*status) {
                    bytes.push(*status);
                }
                *running = Some(*status);
                bytes.push(*first);
                let second = if two { *second } else { 0 };
                if two {
                    bytes.push(second);
                }
                let head = 0x2000_0000
                    | u32::from(*status) << 16
                    | u32::from(*first) << 8
                    | u32::from(second);
                (bytes, vec![[head, 0, 0, 0]])
            }
            Sent::Common {
                status,
                first,
                second,
            } => {
                *running = None;
                let (bytes, first, second) = match status {
                    0xF2 => (vec![*status, *first, *second], *first, *second),
                    0xF1 | 0xF3 => (vec![*status, *first], *first, 0),
                    _ => (vec![*status], 0, 0),
                };
                let head = 0x1000_0000
                    | u32::from(*status) << 16
                    | u32::from(first) << 8
                    | u32::from(second);
                (bytes, vec![[head, 0, 0, 0]])
            }
            Sent::SysEx(data) => {
                *running = None;
                let mut bytes = vec![0xF0];
                bytes.extend(data);
                bytes.push(0xF7);
                (bytes, sysex_packets(data))
            }
        }
    }
}

fn sent() -> impl Strategy<Value = Sent> {
    let data = 0_u8..0x80;
    prop_oneof![
        (0x80_u8..0xF0, data.clone(), data.clone()).prop_map(|(status, first, second)| {
            Sent::Channel {
                status,
                first,
                second,
            }
        }),
        (
            prop::sample::select(vec![0xF1_u8, 0xF2, 0xF3, 0xF6]),
            data.clone(),
            data.clone()
        )
            .prop_map(|(status, first, second)| Sent::Common {
                status,
                first,
                second
            }),
        prop::collection::vec(data, 0..20).prop_map(Sent::SysEx),
    ]
}

/// A stream of messages, with running status where it applies, and the
/// packets it reads as.
fn stream(messages: &[Sent]) -> (Vec<u8>, Vec<[u32; 4]>) {
    let mut running = None;
    let mut bytes = Vec::new();
    let mut packets = Vec::new();
    for message in messages {
        let (b, p) = message.bytes_and_packets(&mut running);
        bytes.extend(b);
        packets.extend(p);
    }
    (bytes, packets)
}

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "tests/bytes.proptest-regressions",
        ))),
        cases: 1024,
        ..ProptestConfig::default()
    })]

    /// Messages read back as their packets, however the reads cut them, and
    /// each packet writes back as its message's bytes.
    #[test]
    fn a_stream_reads_as_its_messages_however_it_is_cut(
        messages in prop::collection::vec(sent(), 0..24),
        cuts in prop::collection::vec(any::<prop::sample::Index>(), 0..8),
    ) {
        let (bytes, expected) = stream(&messages);
        let mut cuts: Vec<usize> = cuts.iter().map(|cut| cut.index(bytes.len() + 1)).collect();
        cuts.sort_unstable();
        let mut stream = ByteStream::new(0);
        let mut packets = Vec::new();
        let mut from = 0;
        for cut in cuts.into_iter().chain([bytes.len()]) {
            stream.read(&bytes[from..cut], |words| packets.push(words));
            from = cut;
        }
        prop_assert_eq!(&packets, &expected);
        // Written back with no running status, the bytes read the same.
        let written: Vec<u8> = packets
            .iter()
            .flat_map(|&words| Bytes::of(words).expect("MIDI 1.0").to_vec())
            .collect();
        prop_assert_eq!(read(&written), expected);
    }

    /// Real-time bytes anywhere in a stream leave every other message as
    /// it was, and go out themselves in the order they came.
    #[test]
    fn real_time_bytes_anywhere_leave_the_rest_alone(
        messages in prop::collection::vec(sent(), 0..16),
        inserts in prop::collection::vec(
            (any::<prop::sample::Index>(), prop::sample::select(vec![0xF8_u8, 0xFA, 0xFB, 0xFC, 0xFE, 0xFF])),
            0..8,
        ),
    ) {
        let (mut bytes, expected) = stream(&messages);
        let mut inserts: Vec<(usize, u8)> = inserts
            .iter()
            .map(|(at, byte)| (at.index(bytes.len() + 1), *byte))
            .collect();
        inserts.sort_by_key(|&(at, _)| at);
        for (shift, &(at, byte)) in inserts.iter().enumerate() {
            bytes.insert(at + shift, byte);
        }
        let packets = read(&bytes);
        let (real_time, rest): (Vec<_>, Vec<_>) =
            packets.into_iter().partition(|words| words[0] >> 28 == 1 && (words[0] >> 16) & 0xFF >= 0xF8);
        prop_assert_eq!(rest, expected);
        let sent: Vec<[u32; 4]> = inserts
            .iter()
            .map(|&(_, byte)| [0x1000_0000 | u32::from(byte) << 16, 0, 0, 0])
            .collect();
        prop_assert_eq!(real_time, sent);
    }
}
