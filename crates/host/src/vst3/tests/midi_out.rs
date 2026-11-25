//! Tests of MIDI output through the VST3 exporter, for notes as note events
//! under the host's ids, a note's expressions, controllers as MIDI 1.0,
//! whole SysEx messages, and a count of what VST3 has no event for.

use ::vst3::Steinberg::int8;

use super::super::BusEvent;
use super::{Sender, note_on, play, playing};
use crate::layout::Stereo;
use crate::{Block, Event, Processor, Status};

/// A MIDI thru, which sends on every message it reads at its frame.
#[derive(Default)]
struct Thru;

impl Processor for Thru {
    type Layout = Stereo;

    fn process(&mut self, block: Block<'_, Stereo>) -> Status {
        let Block {
            midi, mut midi_out, ..
        } = block;
        midi_out.extend(midi);
        Status::Continue
    }
}

/// A processor that sends packets it builds itself:
///
/// - A MIDI 1.0 note-on, and a controller past the block's end.
/// - A note's pitch bend and a timing clock, which VST3 has no event for.
/// - A start, a song position and a MIDI 2.0 note-on at the lowest nonzero
///   velocity.
#[derive(Default)]
struct Raw;

impl Processor for Raw {
    type Layout = Stereo;

    fn process(&mut self, mut block: Block<'_, Stereo>) -> Status {
        for (frame, words) in [
            (0, [0x2090_3C64, 0]),
            (100, [0x20B0_0740, 0]),
            (1, [0x4060_3C00, 0x8000_0000]),
            (2, [0x10F8_0000, 0]),
            (3, [0x10FA_0000, 0]),
            (4, [0x10F2_1020, 0]),
            (5, [0x4090_4000, 0x0001_0000]),
        ] {
            block
                .midi_out
                .send_packet(Event::new(frame, [words[0], words[1], 0, 0]));
        }
        Status::Continue
    }
}

/// A processor that sends two groups' SysEx in interleaved packets, so group
/// 1's begins while group 0's is under way.
#[derive(Default)]
struct TwoGroups;

impl Processor for TwoGroups {
    type Layout = Stereo;

    fn process(&mut self, mut block: Block<'_, Stereo>) -> Status {
        use hapto_ump::SysEx7;
        use hapto_ump::SysExForm::{End, Start};
        for (frame, group, form, data) in [
            (0, 0, Start, [1; 6]),
            (1, 1, Start, [2; 6]),
            (2, 0, End, [3; 6]),
            (3, 1, End, [4; 6]),
        ] {
            let packet = SysEx7::new(group, form, &data);
            block.midi_out.send_packet(packet.event(frame));
        }
        Status::Continue
    }
}

fn near(value: f64, expected: f64) -> bool {
    (value - expected).abs() < 1e-4
}

#[test]
fn a_note_goes_back_out_under_its_hosts_id_and_one_made_up_under_none() {
    let mut host = playing::<Sender<Thru, true>>();
    let off = BusEvent::NoteOff {
        offset: 5,
        channel: 0,
        pitch: 60,
        velocity: 0.25,
        id: 7,
    };
    // Middle C a quarter tone sharp with the host's id 7, then a note
    // without a host id.
    play(
        &mut host,
        &[
            note_on(2, 60, 1.0, 50.0, 7),
            note_on(3, 64, 0.5, 0.0, -1),
            off,
        ],
        &[],
    );
    let sent = host.sent();
    assert_eq!(sent.len(), 3, "{sent:?}");
    assert!(matches!(
        sent[0],
        BusEvent::NoteOn {
            offset: 2,
            channel: 0,
            pitch: 60,
            velocity: 1.0,
            tuning: 50.0,
            id: 7
        }
    ));
    assert!(matches!(
        sent[1],
        BusEvent::NoteOn { offset: 3, pitch: 64, velocity, tuning: 0.0, id: -1, .. }
            if near(f64::from(velocity), 0.5)
    ));
    assert!(matches!(
        sent[2],
        BusEvent::NoteOff { offset: 5, pitch: 60, velocity, id: 7, .. }
            if near(f64::from(velocity), 0.25)
    ));
}

#[test]
fn controllers_go_back_out_as_the_midi_1_values_they_came_in_as() {
    let mut host = playing::<Sender<Thru, true>>();
    let proxy = |controller| host.midi_mapping(0, controller).expect("mapped");
    let (volume, pressure, bend) = (proxy(7), proxy(128), proxy(129));
    let (high, low, entry) = (proxy(101), proxy(100), proxy(6));
    // The pitch bend at 12345 of 16383, volume 100, full pressure and a bend
    // range of 12 semitones, divided as a host divides them.
    let queues: [(u32, &[(i32, f64)]); 6] = [
        (bend, &[(0, 12_345.0 / 16_383.0)]),
        (volume, &[(1, 100.0 / 127.0)]),
        (pressure, &[(3, 1.0)]),
        (entry, &[(4, 12.0 / 127.0)]),
        (high, &[(4, 0.0)]),
        (low, &[(4, 0.0)]),
    ];
    play(&mut host, &[], &queues);
    let sent: Vec<(i32, u8, int8, int8)> = host
        .sent()
        .iter()
        .map(|event| match *event {
            BusEvent::Controller {
                offset,
                channel: 0,
                controller,
                value,
                value2,
            } => (offset, controller, value, value2),
            ref other => panic!("not a controller: {other:?}"),
        })
        .collect();
    // The pitch bend's low 7 bits go first. The range's data entry has no
    // low byte, so it goes out at once, at its own frame, as four
    // controllers.
    assert_eq!(
        sent,
        [
            (0, 129, 57, 96),
            (1, 7, 100, 0),
            (3, 128, 127, 0),
            (4, 101, 0, 0),
            (4, 100, 0, 0),
            (4, 6, 12, 0),
            (4, 38, 0, 0),
        ]
    );
}

#[test]
fn a_note_s_expressions_go_back_out_for_a_note_its_host_named() {
    let mut host = playing::<Sender<Thru, true>>();
    let expression = |kind, value| BusEvent::Expression {
        offset: 4,
        id: 3,
        kind,
        value,
    };
    let volume = BusEvent::Expression {
        offset: 4,
        id: 3,
        kind: 0,
        value: 0.0625,
    };
    let pressure = |offset, pitch, id| BusEvent::Pressure {
        offset,
        channel: 0,
        pitch,
        pressure: 0.5,
        id,
    };
    // A4 a quarter tone sharp, tuned two semitones up and brightened, then a
    // pressed note without a host id.
    let tuned = 0.5 + 2.0 / 240.0;
    play(
        &mut host,
        &[
            note_on(0, 69, 1.0, 50.0, 3),
            expression(2, tuned),
            expression(5, 0.75),
            volume,
            pressure(6, 69, 3),
            note_on(7, 72, 1.0, 0.0, -1),
            pressure(8, 72, -1),
        ],
        &[],
    );
    let sent = host.sent();
    assert_eq!(sent.len(), 7, "{sent:?}");
    assert!(matches!(
        sent[1],
        BusEvent::Expression { offset: 4, id: 3, kind: 2, value } if near(value, tuned)
    ));
    assert!(matches!(
        sent[2],
        BusEvent::Expression { offset: 4, id: 3, kind: 5, value } if near(value, 0.75)
    ));
    assert!(
        matches!(
            sent[3],
            BusEvent::Expression { offset: 4, id: 3, kind: 0, value } if near(value, 0.0625)
        ),
        "the volume back on VST3's scale"
    );
    assert!(matches!(
        sent[4],
        BusEvent::Pressure {
            offset: 6,
            pitch: 69,
            id: 3,
            ..
        }
    ));
    assert!(matches!(
        sent[6],
        BusEvent::Pressure { offset: 8, pitch: 72, pressure, id: -1, .. }
            if near(f64::from(pressure), 0.5)
    ));
    assert_eq!(host.component().lost_out(), 0, "a pressure needs no id");
}

#[test]
fn a_sysex_goes_back_out_whole() {
    let mut host = playing::<Sender<Thru, true>>();
    let short = [0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7];
    let long: Vec<u8> = [0xF0].into_iter().chain(0..20).chain([0xF7]).collect();
    let events = [
        BusEvent::SysEx {
            offset: 1,
            bytes: &short,
        },
        BusEvent::SysEx {
            offset: 3,
            bytes: &long,
        },
    ];
    play(&mut host, &events, &[]);
    let sent = host.sent();
    assert_eq!(sent, events);
}

#[test]
fn a_sysex_begun_on_another_group_while_one_is_under_way_is_dropped() {
    let mut host = playing::<Sender<TwoGroups, true>>();
    play(&mut host, &[], &[]);
    let whole: Vec<u8> = [0xF0]
        .into_iter()
        .chain([1; 6])
        .chain([3; 6])
        .chain([0xF7])
        .collect();
    assert_eq!(
        host.sent(),
        [BusEvent::SysEx {
            offset: 2,
            bytes: &whole
        }]
    );
    assert_eq!(host.component().lost_out(), 1, "group 1's, at its start");
}

#[test]
fn midi_1_a_plugin_builds_goes_out_as_note_events_and_controllers_and_the_rest_is_counted() {
    let mut host = playing::<Sender<Raw, true>>();
    play(&mut host, &[], &[]);
    let sent = host.sent();
    assert_eq!(sent.len(), 5, "{sent:?}");
    assert!(matches!(
        sent[0],
        BusEvent::NoteOn { offset: 0, pitch: 60, velocity, id: -1, .. }
            if near(f64::from(velocity), 100.0 / 127.0)
    ));
    let system = |offset, controller, value, value2| BusEvent::Controller {
        offset,
        channel: 0,
        controller,
        value,
        value2,
    };
    assert_eq!(
        sent[2..4],
        [system(3, 137, 0, 0), system(4, 134, 0x10, 0x20)],
        "a start, and a song position low 7 bits first"
    );
    assert!(
        matches!(
            sent[4],
            BusEvent::NoteOn { offset: 5, pitch: 64, velocity, .. }
                if near(f64::from(velocity), 1.0 / 127.0)
        ),
        "the least velocity a host's MIDI 1.0 keeps as a note-on"
    );
    assert_eq!(
        sent[1],
        BusEvent::Controller {
            offset: 15,
            channel: 0,
            controller: 7,
            value: 64,
            value2: 0
        },
        "past the block's end, at its last frame"
    );
    assert_eq!(
        host.component().lost_out(),
        2,
        "a note's pitch bend and a clock"
    );
}

#[test]
fn a_plugin_that_declares_no_midi_out_writes_none_whatever_it_sends() {
    let mut host = playing::<Sender<Raw, false>>();
    play(&mut host, &[], &[]);
    assert_eq!(host.sent(), []);
    assert_eq!(host.component().lost_out(), 0);
}
