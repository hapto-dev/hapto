//! Fuzzes reading a MIDI 1.0 device's byte stream into packets, in chunks
//! whose size the first byte sets. It checks these properties:
//! - Nothing panics.
//! - The chunk size doesn't change the packets.
//! - The packets write back as bytes that read as the same packets. A SysEx
//!   packet goes out only once it is known whether more SysEx follows, so a
//!   real-time packet can overtake it, and the real-time packets and the rest
//!   are compared separately, each in order.
//!
//! The input ends with an end of SysEx, so no SysEx is left part sent.
#![no_main]

use hapto_ump::{ByteStream, Bytes};
use libfuzzer_sys::fuzz_target;

fn read(bytes: &[u8], cut: usize) -> Vec<[u32; 4]> {
    let mut stream = ByteStream::new(0);
    let mut packets = Vec::new();
    for chunk in bytes.chunks(cut.max(1)) {
        stream.read(chunk, |words| packets.push(words));
    }
    packets
}

fuzz_target!(|data: &[u8]| {
    let Some((&cut, bytes)) = data.split_first() else {
        return;
    };
    let bytes = [bytes, &[0xF7]].concat();
    let packets = read(&bytes, usize::from(cut));
    assert_eq!(packets, read(&bytes, bytes.len()), "a cut changes nothing");
    let written: Vec<u8> = packets
        .iter()
        .flat_map(|&words| Bytes::of(words).expect("every packet is MIDI 1.0").to_vec())
        .collect();
    let again = read(&written, written.len());
    assert_eq!(apart(&again), apart(&packets), "a fixpoint");
});

/// Splits `packets` into the real-time packets and the rest, each in order.
fn apart(packets: &[[u32; 4]]) -> (Vec<[u32; 4]>, Vec<[u32; 4]>) {
    packets
        .iter()
        .partition(|words| words[0] >> 28 == 1 && (words[0] >> 16) & 0xFF >= 0xF8)
}
