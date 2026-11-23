//! The synth's audio thread never allocates: its voices started, stolen,
//! bent, pressed, held by the pedal, released and freed, block after block.

hapto_testkit::install_alloc_gate!();

use hapto_example_synth::Synth;
use hapto_host::layout::Width;
use hapto_host::midi::MidiList;
use hapto_host::{Audio, Block, Event, EventList, MidiOut, Parts, Plugin, ProcessSetup, Processor};
use hapto_testkit::alloc::assert_no_allocations;

/// A block's MIDI 2.0: more notes than the synth has voices, a bend, a
/// note's own pressure and pitch, the pedal down and up, and each note's
/// end.
fn block_midi(round: u32) -> EventList {
    let mut sent = Vec::new();
    for key in 0..20 {
        let key = 40 + key + round % 8;
        let note = key << 8;
        sent.push(Event::new(
            key % 32,
            [0x4090_0000 | note, 0xFFFF_0000, 0, 0],
        ));
        sent.push(Event::new(40, [0x40A0_0000 | note, 0x8000_0000, 0, 0]));
        sent.push(Event::new(44, [0x4000_0003 | note, 0x8A00_0000, 0, 0]));
        sent.push(Event::new(200, [0x4080_0000 | note, 0, 0, 0]));
    }
    sent.push(Event::new(32, [0x40E0_0000, 0xA000_0000, 0, 0]));
    sent.push(Event::new(48, [0x40B0_4000, u32::MAX, 0, 0]));
    sent.push(Event::new(220, [0x40B0_4000, 0, 0, 0]));
    // In time order, as a block hands them over.
    sent.sort_by_key(|event| event.offset);
    let mut packets = EventList::with_capacity(sent.len());
    for event in sent {
        packets.push(event);
    }
    packets
}

#[test]
fn playing_the_voices_never_allocates() {
    let (params, parts) = Parts::for_plugin::<Synth>().expect("valid declarations");
    let mut plugin = Synth::new(params, parts);
    let mut setup = ProcessSetup::new(48_000.0, 256, Width::STEREO_OUT);
    setup.max_events = 128;
    let mut processor = plugin.activate(&setup);
    let mut midi = MidiList::new();
    let rounds: Vec<EventList> = (0..8).map(block_midi).collect();
    let mut out = EventList::with_capacity(0);
    let (mut left, mut right) = (vec![0.0_f32; 256], vec![0.0_f32; 256]);

    let peak = assert_no_allocations("the synth's blocks", || {
        let mut peak = 0.0_f32;
        for packets in rounds.iter().cycle().take(64) {
            midi.read(packets, 256);
            let mut channels: [&mut [f32]; 2] = [&mut left, &mut right];
            let mut audio = Audio::new(&mut channels, 256);
            audio.clear();
            processor.process(Block::new(audio, midi.midi(), MidiOut::new(&mut out)));
            peak = peak.max(left.iter().fold(0.0, |most, sample| most.max(sample.abs())));
        }
        peak
    });
    assert!(peak > 0.1, "the voices played");
}
