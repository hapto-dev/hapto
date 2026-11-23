//! MIDI in through the VST3 exporter: notes named by the host's ids, a
//! note's tuning and expressions, SysEx, and controllers through the hidden
//! proxies a host maps them to, in time order with the notes.

use hapto_midi::{Centred, Message, NoteId, controller};
use hapto_ump::Origin;
use hapto_ump::scale::up_7_to_32;

use super::super::{BusEvent, TestHost};
use crate::layout::Stereo;
use crate::{AuCodes, Block, Category, Parts, Plugin, PluginInfo, ProcessSetup, Processor, Status};

/// Takes MIDI and does nothing with it, so a test reads what the block
/// decoded.
struct Listener;

struct Listening;

impl Processor for Listening {
    type Layout = Stereo;

    fn process(&mut self, _block: Block<'_, Stereo>) -> Status {
        Status::Continue
    }
}

impl Plugin for Listener {
    const INFO: PluginInfo = PluginInfo {
        name: "Listener",
        vendor: "Hapto",
        url: "",
        email: "",
        description: "",
        version: "0.1.0",
        id: "dev.hapto.listener",
        vst3_class_id: [4; 16],
        au_codes: AuCodes {
            subtype: *b"lstn",
            manufacturer: *b"Hapt",
        },
        category: Category::Instrument,
        features: &[],
        layouts: &[],
        midi_input: true,
        midi_output: false,
    };
    type Params = ();
    type Processor = Listening;

    fn new(_: (), _parts: Parts) -> Self {
        Listener
    }

    fn activate(&mut self, _: &ProcessSetup) -> Listening {
        Listening
    }
}

fn running() -> TestHost<Listener> {
    let host = TestHost::<Listener>::new();
    assert_eq!(host.setup(48_000.0, 64), Ok(()));
    assert_eq!(host.activate(), Ok(()));
    assert_eq!(host.start(), Ok(()));
    host
}

/// One block of 16 frames with `events` and `queues`.
fn play(host: &mut TestHost<Listener>, events: &[BusEvent<'_>], queues: &[(u32, &[(i32, f64)])]) {
    let (mut left, mut right) = ([0.0_f32; 16], [0.0_f32; 16]);
    let block = &mut [&mut left[..], &mut right[..]];
    assert_eq!(host.process_events(block, 16, events, queues), Ok(()));
}

/// Each message the last block decoded as its line, with its frame.
fn heard(host: &TestHost<Listener>) -> Vec<(usize, String)> {
    host.component()
        .heard(|midi| {
            midi.iter()
                .map(|(frame, message)| (frame, message.to_string()))
                .collect()
        })
        .expect("active")
}

fn note_on(offset: i32, pitch: i16, velocity: f32, id: i32) -> BusEvent<'static> {
    BusEvent::NoteOn {
        offset,
        channel: 0,
        pitch,
        velocity,
        tuning: 0.0,
        id,
    }
}

#[test]
fn a_note_reaches_the_plugin_as_midi_2_named_by_the_hosts_id() {
    let mut host = running();
    let off = BusEvent::NoteOff {
        offset: 5,
        channel: 0,
        pitch: 60,
        velocity: 0.5,
        id: 7,
    };
    // Two notes on middle C, as VST3 lets a host send them: the note-off
    // that names the first ends it alone.
    play(
        &mut host,
        &[note_on(2, 60, 1.0, 7), note_on(2, 60, 0.5, 9), off],
        &[],
    );
    let notes = host.component().heard(|midi| {
        midi.iter()
            .map(|(frame, message)| match message {
                Message::NoteOn(on) => (frame, "on", on.id, on.velocity),
                Message::NoteOff(off) => (frame, "off", off.id, off.velocity),
                other => panic!("not a note: {other}"),
            })
            .collect::<Vec<_>>()
    });
    let notes = notes.expect("active");
    let ids: Vec<(usize, &str, NoteId)> = notes.iter().map(|n| (n.0, n.1, n.2)).collect();
    assert_eq!(
        ids,
        [
            (2, "on", NoteId::hosts(7)),
            (2, "on", NoteId::hosts(9)),
            (5, "off", NoteId::hosts(7))
        ]
    );
    let velocities: Vec<f32> = notes.iter().map(|n| n.3).collect();
    assert_eq!(velocities[0], 1.0);
    assert!(velocities[1..].iter().all(|v| (v - 0.5).abs() < 1e-4));
}

#[test]
fn a_note_on_s_tuning_is_its_pitch_and_its_expressions_name_it() {
    let mut host = running();
    // A4 a quarter tone sharp, then tuned two semitones up and brightened.
    let on = BusEvent::NoteOn {
        offset: 0,
        channel: 0,
        pitch: 69,
        velocity: 1.0,
        tuning: 50.0,
        id: 3,
    };
    let expression = |kind, value| BusEvent::Expression {
        offset: 4,
        id: 3,
        kind,
        value,
    };
    let unknown = BusEvent::Expression {
        offset: 4,
        id: 42,
        kind: 5,
        value: 1.0,
    };
    play(
        &mut host,
        &[
            on,
            expression(2, 0.5 + 2.0 / 240.0),
            expression(5, 1.0),
            unknown,
        ],
        &[],
    );
    let read = host.component().heard(|midi| {
        let mut found = (None, None, None);
        for (_, message) in midi {
            match message {
                Message::NoteOn(on) => found.0 = Some(on.pitch().semitones()),
                Message::NotePitch { note, pitch } => {
                    found.1 = Some((note.id, pitch.semitones()));
                }
                Message::NoteControl {
                    note,
                    controller,
                    value,
                } => found.2 = Some((note.id, controller, value)),
                other => panic!("unexpected: {other}"),
            }
        }
        found
    });
    let (pitch, tuned, bright) = read.expect("active");
    assert_eq!(pitch, Some(69.5));
    let (id, semitones) = tuned.expect("a note's pitch");
    assert_eq!(id, Some(NoteId::hosts(3)));
    assert!((semitones - 71.5).abs() < 1e-4, "{semitones}");
    assert_eq!(
        bright,
        Some((Some(NoteId::hosts(3)), controller::number::BRIGHTNESS, 1.0)),
        "the unknown note's expression is dropped"
    );
}

#[test]
fn a_sysex_reaches_the_plugin_whole() {
    let mut host = running();
    let long: Vec<u8> = [0xF0].into_iter().chain(0..20).chain([0xF7]).collect();
    let events = [
        BusEvent::SysEx {
            offset: 1,
            bytes: &[0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7],
        },
        BusEvent::SysEx {
            offset: 3,
            bytes: &long,
        },
    ];
    play(&mut host, &events, &[]);
    let data = host.component().heard(|midi| {
        midi.iter()
            .map(|(frame, message)| match message {
                Message::SysEx { data, .. } => (frame, data.to_vec()),
                other => panic!("not a SysEx: {other}"),
            })
            .collect::<Vec<_>>()
    });
    let expected: Vec<u8> = (0..20).collect();
    assert_eq!(
        data.expect("active"),
        [(1, vec![0x7E, 0x7F, 0x06, 0x01]), (3, expected)]
    );
}

#[test]
fn a_controller_comes_through_its_proxy_as_midi_1_upgraded_and_before_a_note_at_its_frame() {
    let mut host = running();
    let volume = host.midi_mapping(0, 7).expect("mapped");
    let pressure = host.midi_mapping(0, 128).expect("mapped");
    let bend = host.midi_mapping(0, 129).expect("mapped");
    // As a host divides a MIDI 1.0 controller: volume 100, full pressure,
    // and the bend at rest.
    let queues: [(u32, &[(i32, f64)]); 3] = [
        (volume, &[(3, 100.0 / 127.0)]),
        (pressure, &[(3, 1.0)]),
        (bend, &[(0, 8192.0 / 16383.0)]),
    ];
    play(&mut host, &[note_on(3, 60, 1.0, -1)], &queues);
    let read = host.component().heard(|midi| {
        let messages: Vec<(usize, String)> = midi
            .iter()
            .map(|(frame, message)| {
                let kind = match message {
                    Message::Bend { value, .. } => {
                        assert_eq!(value, Centred::CENTRE, "exactly at rest");
                        "bend"
                    }
                    Message::Control {
                        controller: controller::number::VOLUME,
                        value,
                        ..
                    } => {
                        let sent = up_7_to_32(100);
                        assert_eq!(value, (f64::from(sent) / f64::from(u32::MAX)) as f32);
                        "volume"
                    }
                    Message::Pressure { value: 1.0, .. } => "pressure",
                    Message::NoteOn(_) => "note",
                    other => panic!("unexpected: {other}"),
                };
                (frame, kind.to_owned())
            })
            .collect();
        let origins: Vec<Origin> = midi.packets().map(|packet| packet.origin).collect();
        (messages, origins)
    });
    let (messages, origins) = read.expect("active");
    let kinds: Vec<(usize, &str)> = messages.iter().map(|(f, k)| (*f, k.as_str())).collect();
    assert_eq!(
        kinds,
        [(0, "bend"), (3, "volume"), (3, "pressure"), (3, "note")]
    );
    assert_eq!(
        origins,
        [Origin::Midi1, Origin::Midi1, Origin::Midi1, Origin::Midi2],
        "a controller is MIDI 1.0 upgraded, so learn narrows it exactly"
    );
}

#[test]
fn a_parameter_s_address_comes_before_its_data_entry_at_one_frame_whatever_the_queues_order() {
    let mut host = running();
    let proxy = |controller| host.midi_mapping(0, controller).expect("mapped");
    let (entry, high, low) = (proxy(6), proxy(101), proxy(100));
    // The bend range, 12 semitones, all at frame 4: the host lists the data
    // entry's queue before the address's.
    let queues: [(u32, &[(i32, f64)]); 3] = [
        (entry, &[(4, 12.0 / 127.0)]),
        (high, &[(4, 0.0)]),
        (low, &[(4, 0.0)]),
    ];
    play(&mut host, &[], &queues);
    let ranges = host.component().heard(|midi| {
        midi.iter()
            .map(|(frame, message)| match message {
                Message::BendRange { semitones, .. } => (frame, semitones),
                other => panic!("unexpected: {other}"),
            })
            .collect::<Vec<_>>()
    });
    // With no low byte, the entry goes out at the block's last frame.
    assert_eq!(ranges.expect("active"), [(15, 12.0)]);
}

#[test]
fn a_data_entry_s_high_byte_comes_before_its_low_byte_at_one_frame_whatever_the_queues_order() {
    let mut host = running();
    let proxy = |controller| host.midi_mapping(0, controller).expect("mapped");
    let (high, low, entry, fine) = (proxy(101), proxy(100), proxy(6), proxy(38));
    let range = |host: &TestHost<Listener>| {
        host.component().heard(|midi| {
            midi.iter()
                .map(|(frame, message)| match message {
                    Message::BendRange { semitones, .. } => (frame, semitones),
                    other => panic!("unexpected: {other}"),
                })
                .collect::<Vec<_>>()
        })
    };
    // The bend range, 2 semitones.
    let first: [(u32, &[(i32, f64)]); 4] = [
        (high, &[(0, 0.0)]),
        (low, &[(0, 0.0)]),
        (entry, &[(0, 2.0 / 127.0)]),
        (fine, &[(0, 0.0)]),
    ];
    play(&mut host, &[], &first);
    assert_eq!(range(&host).expect("active"), [(0, 2.0)]);
    // Then 12 and a half, the host listing the low byte's queue first: it
    // refines its own high byte, not the one before.
    let next: [(u32, &[(i32, f64)]); 2] =
        [(fine, &[(4, 50.0 / 127.0)]), (entry, &[(4, 12.0 / 127.0)])];
    play(&mut host, &[], &next);
    assert_eq!(range(&host).expect("active"), [(4, 12.5)]);
}

#[test]
fn every_channel_s_controllers_map_to_hidden_proxies_listed_after_the_plugin_s_own() {
    let host = running();
    let first = *hapto_params::RESERVED_HOST_IDS.start();
    assert_eq!(host.midi_mapping(0, 0), Some(first));
    assert_eq!(host.midi_mapping(15, 129), Some(first + 16 * 130 - 1));
    assert_eq!(host.midi_mapping(16, 0), None, "no 17th channel");
    assert_eq!(
        host.midi_mapping(0, 130),
        None,
        "no controller past the bend"
    );
    // The listener has no parameter of its own, so the grid is all it lists.
    assert_eq!(host.parameter_count(), 16 * 130);
    let volume = host.parameter(7).expect("listed");
    assert_eq!(
        (volume.id, volume.title.as_str()),
        (first + 7, "Controller 7, channel 1")
    );
    assert!(volume.is_hidden && !volume.can_automate);
    let bend = host.parameter(16 * 130 - 1).expect("listed");
    assert_eq!(bend.title, "Pitch Bend, channel 16");
    assert_eq!(bend.default_normalized, 0.5, "the bend at rest");
    assert_eq!(host.parameter(16 * 130), None);
    // A proxy takes a write and keeps nothing.
    assert_eq!(host.set_normalized(first + 7, 1.0), Ok(()));
    assert_eq!(host.normalized(first + 7), 0.0);
}

#[test]
fn every_channel_offers_the_note_expressions_a_plugin_reads() {
    let host = running();
    let titles: Vec<(u32, String)> = host.note_expressions(0);
    let titles: Vec<(u32, &str)> = titles.iter().map(|(id, t)| (*id, t.as_str())).collect();
    assert_eq!(
        titles,
        [
            (0, "Volume"),
            (1, "Pan"),
            (2, "Tuning"),
            (3, "Vibrato"),
            (4, "Expression"),
            (5, "Brightness")
        ]
    );
    assert_eq!(host.note_expressions(15).len(), 6);
    assert_eq!(host.note_expressions(16), []);
}

#[test]
fn a_plugin_that_takes_no_midi_maps_no_controller_and_lists_no_proxy() {
    let host = super::running(64);
    assert_eq!(host.midi_mapping(0, 7), None);
    assert_eq!(host.note_expressions(0), []);
    assert_eq!(host.parameter_count(), 3, "the probe's own");
}

#[test]
fn a_host_claiming_more_than_the_block_has_room_for_is_read_as_far_as_there_is_room() {
    let mut host = running();
    let volume = host.midi_mapping(0, 7).expect("mapped");
    let points: Vec<(i32, f64)> = (0..3000).map(|point| (point % 16, 0.5)).collect();
    play(&mut host, &[note_on(0, 60, 1.0, 1)], &[(volume, &points)]);
    let (count, dropped) = host
        .component()
        .heard(|midi| (midi.len(), midi.dropped()))
        .expect("active");
    // The block's room, 2048 packets, all controllers, which come first;
    // the rest of the points and the note are counted.
    assert_eq!((count, dropped), (2048, 3000 - 2048 + 1));
}

#[test]
fn a_note_expression_from_before_an_activation_names_no_note() {
    let mut host = running();
    play(&mut host, &[note_on(0, 60, 1.0, 5)], &[]);
    assert_eq!(host.stop(), Ok(()));
    assert_eq!(host.deactivate(), Ok(()));
    assert_eq!(host.activate(), Ok(()));
    assert_eq!(host.start(), Ok(()));
    let bright = BusEvent::Expression {
        offset: 0,
        id: 5,
        kind: 5,
        value: 1.0,
    };
    play(&mut host, &[bright], &[]);
    assert_eq!(heard(&host), []);
}
