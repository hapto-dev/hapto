//! Instruction counts for learn's dispatch of one block of MIDI on the audio
//! thread, with a table of mappings published. Run by CI with iai-callgrind,
//! which compares them with the pull request's base.

#![allow(missing_docs, reason = "the benchmark macros generate public items")]

#[path = "../tests/common/mod.rs"]
mod common;

use std::hint::black_box;

use hapto_learn::{Config, Control, Learn, LearnAudio, Mapping, Source};
use hapto_params::{Kind, ParamDecl, ParamSetBuilder, Range};
use hapto_ump::{Event, EventList, Midi1Message, Midi1Voice, Upgrade};
use iai_callgrind::{library_benchmark, library_benchmark_group, main};

const TARGETS: usize = 64;

/// A learn with every target mapped to a controller or a note, and the
/// audio side that dispatches.
fn mapped() -> (Learn, LearnAudio) {
    let mut builder = ParamSetBuilder::new();
    for index in 0..TARGETS {
        let key: &'static str = Box::leak(format!("p{index}").into_boxed_str());
        builder
            .add(ParamDecl::new(
                key,
                key,
                Kind::Continuous(
                    Range::new(20.0, 20_000.0, 0.0, Some(1_000.0)).expect("a valid range"),
                ),
                1_000.0,
            ))
            .expect("valid");
    }
    let set = builder.build();
    let (mut learn, audio) = Learn::new(&set, Config::default());
    for (index, id) in set.ids().enumerate() {
        let control = match index % 3 {
            0 => Control::Cc([1, 2, 3, 4, 5, 7, 8, 9][index / 3 % 8]),
            1 => Control::Nrpn(300 + (index / 3 % 4) as u16),
            _ => Control::Note(60 + (index / 3 % 4) as u8),
        };
        let source = Source {
            control,
            group: None,
            channel: Some(1),
        };
        learn.map(id, Mapping::new(source)).expect("legal");
    }
    (learn, audio)
}

/// Returns a block of controller moves, an NRPN with its data, and notes,
/// upgraded to MIDI 2.0 as an exporter does.
fn block() -> EventList {
    let on = |message| {
        Midi1Voice {
            group: 0,
            channel: 1,
            message,
        }
        .event(0)
    };
    let cc = |controller, value| on(Midi1Message::ControlChange { controller, value });
    let mut sent = Vec::new();
    for value in [0, 64, 127] {
        sent.extend((1..=8).map(|controller| cc(controller, value)));
        sent.extend([cc(99, 2), cc(98, 44), cc(6, value), cc(38, value)]);
        sent.push(on(Midi1Message::NoteOn {
            note: 60,
            velocity: value.max(1),
        }));
        sent.push(on(Midi1Message::NoteOff {
            note: 60,
            velocity: 0,
        }));
    }
    sent.push(Event::new(0, [0x3016_7E7F, 0x0D70_0201, 0, 0]));
    common::upgraded(&mut Upgrade::new(), &sent)
}

#[library_benchmark]
#[bench::first_block(mapped(), block())]
fn dispatch(mut state: (Learn, LearnAudio), events: EventList) -> ((Learn, LearnAudio), EventList) {
    state.1.process(black_box(&events));
    (state, events)
}

library_benchmark_group!(name = learn; benchmarks = dispatch);
main!(library_benchmark_groups = learn);
