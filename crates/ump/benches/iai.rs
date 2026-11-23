//! Instruction counts for reading UMP: splitting a stream into packets,
//! decoding a channel voice message, and upgrading a block of MIDI 1.0. Run
//! by CI with iai-callgrind, which compares them with the pull request's
//! base.

#![allow(missing_docs, reason = "the benchmark macros generate public items")]

use std::hint::black_box;

use hapto_ump::{ChannelVoice, Event, EventList, Midi1Voice, Packets, Upgrade};
use iai_callgrind::{library_benchmark, library_benchmark_group, main};

/// A stream of one packet of every size, repeated.
fn stream() -> Vec<u32> {
    let packets: [&[u32]; 5] = [
        &[0x2090_3C7F],
        &[0x4090_3C00, 0xC000_0000],
        &[0x0000_0000],
        &[0x3016_7E7F, 0x0D70_0201],
        &[0xF000_0000, 0, 0, 0],
    ];
    (0..64)
        .flat_map(|_| packets.iter().copied().flatten().copied())
        .collect()
}

#[library_benchmark]
#[bench::mixed(stream())]
fn split_a_stream(words: Vec<u32>) -> Vec<u32> {
    black_box(Packets::new(black_box(&words), 0).count());
    words
}

#[library_benchmark]
#[bench::note_on([0x4090_3C00, 0xC000_0000, 0, 0])]
#[bench::controller([0x40B1_0200, 0x8000_0000, 0, 0])]
#[bench::assignable_controller([0x4031_0205, 0x8000_0000, 0, 0])]
fn decode_a_voice(words: [u32; 4]) -> Option<ChannelVoice> {
    black_box(ChannelVoice::decode(black_box(words)))
}

#[library_benchmark]
#[bench::note_on([0x2090_3C7F, 0, 0, 0])]
fn decode_a_midi1_voice(words: [u32; 4]) -> Option<Midi1Voice> {
    black_box(Midi1Voice::decode(black_box(words)))
}

/// A block of MIDI 1.0 as a keyboard sends it: notes, a controller, a pitch
/// bend and a whole NRPN, 16 times over.
fn midi1_block() -> (Upgrade, Vec<Event>, EventList) {
    let packets = [
        0x2090_3C7F,
        0x20B0_0164,
        0x20E0_0040,
        0x20B0_6302,
        0x20B0_6205,
        0x20B0_0640,
        0x20B0_2601,
        0x2080_3C00,
    ];
    let events = (0..16)
        .flat_map(|_| packets)
        .zip(0..)
        .map(|(head, offset)| Event::new(offset, [head, 0, 0, 0]))
        .collect();
    (Upgrade::new(), events, EventList::with_capacity(256))
}

#[library_benchmark]
#[bench::keyboard(midi1_block())]
fn upgrade_a_block((mut upgrade, events, mut out): (Upgrade, Vec<Event>, EventList)) -> EventList {
    upgrade.block(black_box(&events), 255, &mut out);
    black_box(out)
}

library_benchmark_group!(
    name = ump;
    benchmarks = split_a_stream, decode_a_voice, decode_a_midi1_voice, upgrade_a_block
);
main!(library_benchmark_groups = ump);
