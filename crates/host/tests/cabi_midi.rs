//! MIDI through the C interface, as the JUCE adapter passes it: packets in,
//! MIDI 1.0 or 2.0, upgraded before the plugin reads them, and packets out,
//! downgraded for a caller that takes MIDI 1.0 alone.

#![cfg(all(feature = "cabi", feature = "test-host"))]

use hapto_host::cabi::{HaptoPacket, MidiBlock, TestHost};
use hapto_host::layout::{Stereo, Width};
use hapto_host::{
    AuCodes, Block, Category, Parts, Plugin, PluginInfo, ProcessSetup, Processor, Status,
};

/// Passes every message it reads on, at its frame: MIDI thru.
struct Thru;

struct ThruProcessor;

impl Processor for ThruProcessor {
    type Layout = Stereo;

    fn process(&mut self, block: Block<'_, Stereo>) -> Status {
        let Block {
            midi, mut midi_out, ..
        } = block;
        midi_out.extend(midi);
        Status::Continue
    }
}

impl Plugin for Thru {
    const INFO: PluginInfo = PluginInfo {
        name: "Thru",
        vendor: "Hapto",
        url: "",
        email: "",
        description: "",
        version: "0.1.0",
        id: "dev.hapto.thru",
        vst3_class_id: [3; 16],
        au_codes: AuCodes {
            subtype: *b"thru",
            manufacturer: *b"Hapt",
        },
        category: Category::Effect,
        features: &[],
        layouts: &[],
        midi_input: true,
        midi_output: true,
    };
    type Params = ();
    type Processor = ThruProcessor;

    fn new(_: (), _parts: Parts) -> Self {
        Thru
    }

    fn activate(&mut self, _: &ProcessSetup) -> ThruProcessor {
        ThruProcessor
    }
}

/// Sends on every packet it gets, whatever its type.
struct Echo;

struct EchoProcessor;

impl Processor for EchoProcessor {
    type Layout = Stereo;

    fn process(&mut self, mut block: Block<'_, Stereo>) -> Status {
        block.midi_out.extend(block.midi.packets());
        Status::Continue
    }
}

impl Plugin for Echo {
    const INFO: PluginInfo = PluginInfo {
        name: "Echo",
        id: "dev.hapto.echo",
        vst3_class_id: [6; 16],
        au_codes: AuCodes {
            subtype: *b"echo",
            manufacturer: *b"Hapt",
        },
        ..Thru::INFO
    };
    type Params = ();
    type Processor = EchoProcessor;

    fn new(_: (), _parts: Parts) -> Self {
        Echo
    }

    fn activate(&mut self, _: &ProcessSetup) -> EchoProcessor {
        EchoProcessor
    }
}

fn packet(frame: u32, first: u32) -> HaptoPacket {
    HaptoPacket {
        frame,
        words: [first, 0, 0, 0],
    }
}

const EMPTY: HaptoPacket = HaptoPacket {
    frame: 0,
    words: [0; 4],
};

/// One block of 16 frames with `midi_in`, room for `room` packets out, and
/// what came out.
fn play<P: Plugin>(
    host: &TestHost<P>,
    midi_in: &[HaptoPacket],
    room: usize,
    midi_1_out: bool,
) -> Vec<HaptoPacket> {
    let (mut left, mut right) = ([0.0_f32; 16], [0.0_f32; 16]);
    let mut out = vec![EMPTY; room];
    let midi = MidiBlock {
        midi_in,
        midi_out: &mut out,
        midi_1_out,
    };
    let (_, sent) = host.process_with_midi(&mut [&mut left[..], &mut right[..]], 16, midi);
    out.truncate(sent);
    out
}

fn running() -> TestHost<Thru> {
    let host = TestHost::<Thru>::new();
    assert_eq!(host.activate(48_000.0, 16, Width::STEREO), Ok(()));
    host
}

#[test]
fn midi_1_in_is_upgraded_and_goes_out_as_midi_2() {
    let host = running();
    // A MIDI 1.0 note-on at frame 3, as a MIDI 1.0 host sends it.
    let out = play(&host, &[packet(3, 0x2090_3C7F)], 8, false);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].frame, 3);
    assert_eq!(
        out[0].words,
        [0x4090_3C00, 0xFFFF_0000, 0, 0],
        "MIDI 2.0's note-on"
    );
}

#[test]
fn a_caller_that_takes_midi_1_alone_gets_it_back_as_it_sent_it() {
    let host = running();
    let sent = [
        packet(0, 0x2090_3C7F),
        packet(5, 0x20B0_0764),
        packet(9, 0x2080_3C40),
    ];
    let out = play(&host, &sent, 8, true);
    let words: Vec<(u32, u32)> = out.iter().map(|p| (p.frame, p.words[0])).collect();
    assert_eq!(
        words,
        [(0, 0x2090_3C7F), (5, 0x20B0_0764), (9, 0x2080_3C40)]
    );
}

#[test]
fn a_caller_that_takes_midi_1_alone_gets_no_packet_midi_1_has_no_place_for() {
    let host = TestHost::<Echo>::new();
    assert_eq!(host.activate(48_000.0, 16, Width::STEREO), Ok(()));
    // A note-on, then flex data, a utility message and SysEx8, which MIDI
    // 1.0 has no place for.
    let sent = [
        packet(0, 0x2090_3C7F),
        packet(1, 0xD010_0000),
        packet(2, 0x0020_0010),
        packet(3, 0x5001_0000),
    ];
    let types =
        |out: &[HaptoPacket]| -> Vec<u32> { out.iter().map(|p| p.words[0] >> 28).collect() };
    assert_eq!(types(&play(&host, &sent, 8, true)), [0x2]);
    assert_eq!(
        types(&play(&host, &sent, 8, false)),
        [0x4, 0xD, 0x0, 0x5],
        "every packet, to a caller that takes MIDI 2.0"
    );
}

#[test]
fn past_the_room_out_a_packet_is_dropped_and_the_count_says_what_fit() {
    let host = running();
    let sent = [packet(0, 0x2090_3C7F), packet(1, 0x2090_3E7F)];
    assert_eq!(play(&host, &sent, 1, true).len(), 1);
    // No room at all: nothing is written, and the count says so.
    assert_eq!(play(&host, &sent, 0, true).len(), 0);
}

#[test]
fn midi_2_in_and_a_sysex_go_through_and_back_out() {
    let host = running();
    let sent = [
        // A MIDI 2.0 note-on, and a SysEx in two packets.
        HaptoPacket {
            frame: 1,
            words: [0x4090_3C00, 0x8000_0000, 0, 0],
        },
        HaptoPacket {
            frame: 2,
            words: [0x3016_7E7F, 0x0601_0203, 0, 0],
        },
        HaptoPacket {
            frame: 2,
            words: [0x3032_0405, 0, 0, 0],
        },
    ];
    let out = play(&host, &sent, 8, false);
    let heads: Vec<(u32, u32)> = out.iter().map(|p| (p.frame, p.words[0])).collect();
    assert_eq!(
        heads,
        [(1, 0x4090_3C00), (2, 0x3016_7E7F), (2, 0x3032_0405)],
        "the note as it came, and the SysEx whole again"
    );
}

#[test]
fn a_block_with_no_midi_or_no_processor_sends_none() {
    let host = running();
    assert_eq!(play(&host, &[], 4, false).len(), 0);
    host.deactivate();
    assert_eq!(play(&host, &[packet(0, 0x2090_3C7F)], 4, false).len(), 0);
}
