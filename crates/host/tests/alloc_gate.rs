//! What hapto-host runs over and over does not allocate: the per-block event
//! list a format exporter fills and a processor reads, a block's spans with
//! their messages, a block through an
//! exporter with learn's audio side in it, a block of the host's MIDI
//! through the VST3 exporter, in and back out, the standalone exporter's
//! audio side around
//! the engine, and a plugin's idle tick when nothing changed, which every
//! instance runs about 60 times a second.

hapto_testkit::install_alloc_gate!();

use hapto_host::layout::{AnyLayout, Width};
use hapto_host::{
    AuCodes, Block, Category, Driver, Event, EventList, Parts, Plugin, PluginInfo, ProcessSetup,
    Processor, Status,
};
use hapto_learn::{Config, Control, Mapping, Source, Takes};
use hapto_params::{Declarer, Float, FloatDecl, Params};
use hapto_testkit::alloc::assert_no_allocations;
use hapto_undo::Bound;

#[test]
fn walking_a_blocks_spans_does_not_allocate() {
    use hapto_host::AudioBuf;
    use hapto_host::layout::Stereo;
    use hapto_host::midi::MidiList;

    let mut packets = EventList::with_capacity(8);
    for offset in [0, 3, 3, 40, 255] {
        packets.push(Event::new(offset, [0x40D0_0000, offset, 0, 0]));
    }
    let mut list = MidiList::new();
    let mut buffer = AudioBuf::<Stereo>::new(256);
    let seen = assert_no_allocations("a block's spans walked", || {
        let mut seen = (0, 0);
        for most in [usize::MAX, 64, 1] {
            list.read(&packets, 256);
            for span in hapto_host::Spans::new(buffer.audio(), list.midi()).at_most(most) {
                seen.0 += 1;
                seen.1 += span.midi.messages().len();
            }
        }
        seen
    });
    // Spans at frames 0, 3, 40 and 255: four whole; seven at most 64 frames
    // long, the one from 40 cut at 104, 168 and 232; 256 one frame each;
    // every message once each time.
    assert_eq!(seen, (4 + 7 + 256, 3 * 5));
}

#[test]
fn filling_and_reading_a_block_of_events_does_not_allocate() {
    let mut events = EventList::with_capacity(512);
    assert_no_allocations("an event list over many blocks, overflow included", || {
        for block in 0..64u32 {
            events.clear();
            for offset in 0..600u32 {
                events.push(Event::new(offset, [0x2090_3C00 | (block & 0x7F), 0, 0, 0]));
            }
            let sum: u32 = events.as_slice().iter().map(|event| event.offset).sum();
            std::hint::black_box((sum, events.dropped()));
        }
    });
}

struct Gain {
    _gain: Float,
}

impl Params for Gain {
    fn declare(p: &mut Declarer) -> Self {
        Gain {
            _gain: p.add(FloatDecl::new("gain", "Gain", 0.0..=1.0, 0.5)),
        }
    }
}

struct Silent;

impl Processor for Silent {
    type Layout = AnyLayout;

    fn process(&mut self, _: Block<'_, AnyLayout>) -> Status {
        Status::Sleep
    }
}

/// Sends on every message it reads, at its frame.
struct Echo;

impl Processor for Echo {
    type Layout = AnyLayout;

    fn process(&mut self, block: Block<'_, AnyLayout>) -> Status {
        let Block {
            midi, mut midi_out, ..
        } = block;
        midi_out.extend(midi);
        Status::Continue
    }
}

/// A plugin that takes MIDI and sends what it reads back out.
struct Echoing;

impl Plugin for Echoing {
    const INFO: PluginInfo = PluginInfo {
        id: "dev.hapto.echoing",
        midi_output: true,
        ..Listening::INFO
    };
    type Params = Gain;
    type Processor = Echo;

    fn new(_: Gain, _: Parts) -> Self {
        Echoing
    }

    fn activate(&mut self, _: &ProcessSetup) -> Echo {
        Echo
    }
}

/// A plugin that takes MIDI, for parts with learn on; its parts are used
/// with no plugin built.
struct Listening;

impl Plugin for Listening {
    const INFO: PluginInfo = PluginInfo {
        name: "Listening",
        vendor: "Hapto",
        url: "",
        email: "",
        description: "",
        version: "0.1.0",
        id: "dev.hapto.listening",
        vst3_class_id: [0; 16],
        au_codes: AuCodes {
            subtype: *b"lstn",
            manufacturer: *b"Hapt",
        },
        category: Category::Effect,
        features: &[],
        layouts: &[Width::STEREO],
        midi_input: true,
        midi_output: false,
    };
    type Params = Gain;
    type Processor = Silent;

    fn new(_: Gain, _: Parts) -> Self {
        Listening
    }

    fn activate(&mut self, _: &ProcessSetup) -> Silent {
        Silent
    }
}

#[test]
fn an_idle_tick_of_a_plugin_s_parts_does_not_allocate() {
    let (_params, mut parts) = Parts::for_plugin::<Listening>().expect("valid declarations");
    let _history = parts.undo(Bound::default());
    parts.midi_learn(Config::default());
    let driver = Driver::of(&parts);
    let _audio = driver.learn_audio();
    driver.tick();
    assert_no_allocations("idle ticks of parts with undo and learn on", || {
        for _ in 0..64 {
            driver.tick();
        }
    });
}

#[test]
fn an_idle_tick_with_ten_thousand_parameters_made_at_run_time_does_not_allocate() {
    let (_params, mut parts) = Parts::for_plugin::<Listening>().expect("valid declarations");
    let _history = parts.undo(Bound::default());
    parts.midi_learn(Config::default());
    let driver = Driver::of(&parts);
    let _audio = driver.learn_audio();
    let made: Vec<Float> = (0..10_000)
        .map(|node| {
            let decl = FloatDecl::new("level", "Level", 0.0..=1.0, 0.5);
            parts.add(&format!("node/{node}"), decl)
        })
        .collect();
    // A few learned, so learn's poll walks them.
    for (number, param) in (20..28).zip(&made) {
        let mapping = Mapping::new(Source::any(Control::Cc(number)));
        parts.learn().map(param, mapping).expect("legal");
    }
    driver.tick();
    assert_no_allocations(
        "idle ticks with ten thousand parameters made at run time",
        || {
            for _ in 0..64 {
                driver.tick();
            }
        },
    );
}

/// A plugin that keeps its parts, with undo and learn on, so the engine
/// ticks them.
#[cfg(any(all(feature = "cabi", feature = "test-host"), feature = "standalone"))]
struct Kept {
    _parts: Parts,
}

#[cfg(any(all(feature = "cabi", feature = "test-host"), feature = "standalone"))]
impl Plugin for Kept {
    const INFO: PluginInfo = Listening::INFO;
    type Params = Gain;
    type Processor = Silent;

    fn new(_: Gain, mut parts: Parts) -> Self {
        parts.undo(Bound::default());
        parts.midi_learn(Config::default());
        Kept { _parts: parts }
    }

    fn activate(&mut self, _: &ProcessSetup) -> Silent {
        Silent
    }
}

#[cfg(all(feature = "cabi", feature = "test-host"))]
#[test]
fn an_idle_tick_through_the_engine_does_not_allocate() {
    let host = hapto_host::cabi::TestHost::<Kept>::new();
    host.idle();
    assert_no_allocations(
        "the engine's idle ticks of a plugin that keeps its parts",
        || {
            for _ in 0..64 {
                host.idle();
            }
        },
    );
    assert_eq!(host.faults(), 0);
}

#[cfg(all(feature = "cabi", feature = "test-host"))]
#[test]
fn a_block_through_the_engine_with_learn_on_does_not_allocate() {
    let host = hapto_host::cabi::TestHost::<Kept>::new();
    assert_eq!(host.activate(48_000.0, 256, Width::STEREO), Ok(()));
    let (mut left, mut right) = (vec![0.25f32; 600], vec![-0.25f32; 600]);
    host.process_in_place(&mut [&mut left, &mut right], 256);
    assert_no_allocations(
        "blocks that run learn's audio side, then the processor",
        || {
            for frames in [256, 600, 1, 255] {
                host.process_in_place(&mut [&mut left, &mut right], frames);
            }
        },
    );
    assert_eq!(host.faults(), 0);
}

/// A plugin with learn on, a controller mapped and its target armed, which
/// sends what it reads back out.
#[cfg(all(feature = "cabi", feature = "test-host"))]
struct Learned {
    _parts: Parts,
}

#[cfg(all(feature = "cabi", feature = "test-host"))]
impl Plugin for Learned {
    const INFO: PluginInfo = PluginInfo {
        id: "dev.hapto.learned",
        midi_output: true,
        ..Listening::INFO
    };
    type Params = Gain;
    type Processor = Echo;

    fn new(_: Gain, mut parts: Parts) -> Self {
        parts.midi_learn(Config::default());
        let gain = parts.set().id("gain").expect("declared");
        let volume = Mapping::new(Source::any(Control::Cc(7)));
        parts.learn().map(gain, volume).expect("legal");
        parts.learn().arm(gain, Takes::CONTROLLER).expect("known");
        Learned { _parts: parts }
    }

    fn activate(&mut self, _: &ProcessSetup) -> Echo {
        Echo
    }
}

#[cfg(all(feature = "cabi", feature = "test-host"))]
#[test]
fn midi_through_the_c_interface_with_learn_mapped_and_armed_does_not_allocate() {
    use hapto_host::cabi::{HaptoPacket, MidiBlock, TestHost};

    let host = TestHost::<Learned>::new();
    assert_eq!(host.activate(48_000.0, 256, Width::STEREO), Ok(()));
    let packet = |frame, first, second| HaptoPacket {
        frame,
        words: [first, second, 0, 0],
    };
    // MIDI 1.0 as a JUCE host hands it over: a note, the mapped controller,
    // an NRPN whose high byte the upgrade holds to the block's end, and a
    // SysEx; then a MIDI 2.0 note-off.
    let midi_in = [
        packet(0, 0x2090_3C7F, 0),
        packet(1, 0x20B0_0764, 0),
        packet(2, 0x20B0_6302, 0),
        packet(2, 0x20B0_622C, 0),
        packet(3, 0x20B0_0640, 0),
        packet(4, 0x3016_7E7F, 0x0601_0203),
        packet(4, 0x3032_0405, 0),
        packet(5, 0x4080_3C00, 0x8000_0000),
    ];
    let mut out = [HaptoPacket::NONE; 64];
    let (mut left, mut right) = (vec![0.25f32; 600], vec![-0.25f32; 600]);
    let mut sent = 0;
    assert_no_allocations(
        "blocks of MIDI through the C interface, learn reading them, sent back",
        || {
            for midi_1_out in [false, true] {
                for frames in [256, 600, 1, 255] {
                    let midi = MidiBlock {
                        midi_in: &midi_in,
                        midi_out: &mut out,
                        midi_1_out,
                    };
                    let block = &mut [&mut left[..frames], &mut right[..frames]];
                    sent += host.process_with_midi(block, frames, midi).1;
                }
            }
        },
    );
    assert!(sent > 0, "it sent what it read");
    assert_eq!(host.faults(), 0);
}

#[cfg(all(feature = "vst3", feature = "test-host"))]
#[test]
fn a_block_of_notes_controllers_and_sysex_through_the_vst3_exporter_does_not_allocate() {
    vst3_midi_blocks::<Listening>("blocks with MIDI in through the VST3 exporter");
}

#[cfg(all(feature = "vst3", feature = "test-host"))]
#[test]
fn a_plugin_sending_its_midi_back_through_the_vst3_exporter_does_not_allocate() {
    vst3_midi_blocks::<Echoing>("blocks with MIDI in and back out through the VST3 exporter");
}

/// Blocks with a note, its expression and pressure, a SysEx and
/// controllers through the VST3 exporter, the first after activation among
/// them.
#[cfg(all(feature = "vst3", feature = "test-host"))]
fn vst3_midi_blocks<P: Plugin>(what: &str) {
    use hapto_host::vst3::{BusEvent, TestHost};

    let mut host = TestHost::<P>::new();
    host.setup(48_000.0, 256).expect("set up");
    host.activate().expect("active");
    host.start().expect("started");
    let volume = host.midi_mapping(0, 7).expect("mapped");
    let bend = host.midi_mapping(0, 129).expect("mapped");
    let sysex = [0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7];
    let events = [
        BusEvent::NoteOn {
            offset: 0,
            channel: 0,
            pitch: 60,
            velocity: 1.0,
            tuning: 25.0,
            id: 1,
        },
        BusEvent::Expression {
            offset: 2,
            id: 1,
            kind: 2,
            value: 0.6,
        },
        BusEvent::Pressure {
            offset: 3,
            channel: 0,
            pitch: 60,
            pressure: 0.5,
            id: 1,
        },
        BusEvent::SysEx {
            offset: 4,
            bytes: &sysex,
        },
        BusEvent::NoteOff {
            offset: 5,
            channel: 0,
            pitch: 60,
            velocity: 0.5,
            id: 1,
        },
    ];
    let volume_points: &[(i32, f64)] = &[(1, 0.5), (6, 1.0)];
    let bend_points: &[(i32, f64)] = &[(0, 0.5)];
    let queues = [(volume, volume_points), (bend, bend_points)];
    let (mut left, mut right) = (vec![0.25f32; 600], vec![-0.25f32; 600]);
    assert_no_allocations(what, || {
        for frames in [256, 256, 600, 1, 255] {
            let block = &mut [&mut left[..frames], &mut right[..frames]];
            host.process_events(block, frames, &events, &queues)
                .expect("played");
        }
    });
    assert_eq!(host.faults(), 0);
}

#[cfg(feature = "standalone")]
#[test]
fn a_block_through_the_standalone_exporter_does_not_allocate() {
    use hapto_host::standalone::{MAX_CHANNELS, Standalone};

    let app = Standalone::<Kept>::new();
    let mut audio = app.audio();
    let input = vec![0.25f32; 600];
    let (mut left, mut right) = (vec![0.0f32; 600], vec![0.0f32; 600]);
    let mut wide = vec![vec![0.0f32; 64]; MAX_CHANNELS + 1];
    let mut outputs: Vec<&mut [f32]> = wide.iter_mut().map(|c| &mut c[..]).collect();
    // A controller, and an NRPN whose high byte the upgrade holds to the
    // block's end, past the block's end for the shortest.
    let midi = [0x00B0_0764, 0x00B0_6302, 0x00B0_622C, 0x00B0_0640]
        .map(|bytes: u32| hapto_host::Event::new(3, [0x2000_0000 | bytes, 0, 0, 0]));
    assert_no_allocations("a block before the activation, which is silence", || {
        audio.process(
            &[&input, &input],
            &mut [&mut left, &mut right],
            600,
            &[],
            |_| {},
        );
    });
    assert_eq!(app.activate(48_000.0, 256, Width::STEREO), Ok(()));
    assert_no_allocations(
        "blocks through the standalone exporter, the first after activation on",
        || {
            for frames in [256, 600, 1, 255] {
                let inputs = [&input[..frames], &input[..frames]];
                audio.process(
                    &inputs,
                    &mut [&mut left[..frames], &mut right[..frames]],
                    frames,
                    &midi,
                    |_| {},
                );
            }
            // An input shorter than the outputs, inputs that share part of
            // one buffer, and more outputs than the most.
            audio.process(
                &[&input[..100], &input[..100]],
                &mut [&mut left, &mut right],
                600,
                &[],
                |_| {},
            );
            let shifted = [&input[..256], &input[1..257]];
            audio.process(
                &shifted,
                &mut [&mut left[..256], &mut right[..256]],
                256,
                &[],
                |_| {},
            );
            audio.process(&[], &mut outputs, 64, &[], |_| {});
        },
    );
    assert_eq!(app.faults(), 0);
    drop(audio);
}
