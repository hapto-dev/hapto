use std::num::NonZeroUsize;
use std::sync::Arc;

use hapto_learn::{Config, Control, Learn, Learned, Mapping, Source, Takes};
use hapto_params::{Kind, ParamDecl, ParamId, ParamSet};
use hapto_ump::{
    ChannelMessage, ChannelVoice, EventList, Midi1Message, Midi1Voice, Origin, Upgrade,
};

use super::{Active, Rank, fill, pieces};
use crate::layout::{AnyLayout, Width};
use crate::{Block, Event, Processor, Status};

/// What one piece handed the processor: its frames, each event's offset and
/// tag, and the drops its list reported.
#[derive(Debug, PartialEq)]
struct Piece {
    frames: usize,
    events: Vec<(u32, u32)>,
    dropped: u64,
}

/// Records each piece, and sends every event it gets back at the same
/// frame of its piece.
#[derive(Debug, Default)]
struct Echo {
    pieces: Vec<Piece>,
}

impl Processor for Echo {
    type Layout = AnyLayout;

    fn process(&mut self, mut block: Block<'_, AnyLayout>) -> Status {
        let events = block.midi.packets().map(|event| tagged(&event)).collect();
        self.pieces.push(Piece {
            frames: block.audio.frames(),
            events,
            dropped: block.midi.dropped(),
        });
        block.midi_out.extend(block.midi.packets());
        Status::Continue
    }
}

/// A note-on at `offset` whose velocity is `tag`, so a test can follow it.
fn note(offset: u32, tag: u32) -> Event {
    Event::new(offset, [0x2090_3C00 | tag, 0, 0, 0])
}

fn tagged(event: &Event) -> (u32, u32) {
    (event.offset, event.words[0] & 0x7F)
}

/// Room for 6 events in and 5 out, in pieces of up to 64 frames.
fn audio() -> Active<Echo> {
    let max_frames = NonZeroUsize::new(64).expect("not 0");
    Active::around(Echo::default(), Width::new(0, 0), max_frames, 6, 5)
}

#[test]
fn each_piece_gets_its_own_events_and_what_it_sends_lands_at_the_block_s_frames() {
    let mut audio = audio();
    // In delivery order. One is past the block's end, and the last has no
    // room in the block's list.
    for (offset, tag) in [
        (10, 1),
        (63, 2),
        (64, 3),
        (100, 4),
        (149, 5),
        (400, 6),
        (20, 7),
    ] {
        audio.events.push(note(offset, tag));
    }
    // SAFETY: a layout of no channels, so no buffer is read or written.
    let status = unsafe { pieces(&mut audio, 150, &[], &[]) };
    assert_eq!(status, Status::Continue);

    let piece = |frames, events: &[(u32, u32)], dropped| Piece {
        frames,
        events: events.to_vec(),
        dropped,
    };
    assert_eq!(
        audio.processor.pieces,
        [
            piece(64, &[(10, 1), (63, 2)], 1),
            piece(64, &[(0, 3), (36, 4)], 0),
            // The event past the end lands at the block's last frame.
            piece(22, &[(21, 5), (21, 6)], 0),
        ],
        "each piece its own events, from its own start; the first reports \
         the block's drops"
    );
    let sent: Vec<_> = audio.out_events.as_slice().iter().map(tagged).collect();
    assert_eq!(sent, [(10, 1), (63, 2), (64, 3), (100, 4), (149, 5)]);
    assert_eq!(audio.out_events.dropped(), 1, "the sixth had no room");
}

#[test]
fn a_block_of_one_piece_hands_its_events_on_and_the_next_starts_clean() {
    let mut audio = audio();
    audio.events.push(note(5, 1));
    audio.events.push(note(80, 2));
    // SAFETY: as above.
    unsafe { pieces(&mut audio, 32, &[], &[]) };
    audio.events.clear();
    // SAFETY: as above.
    unsafe { pieces(&mut audio, 16, &[], &[]) };

    assert_eq!(
        audio.processor.pieces,
        [
            Piece {
                frames: 32,
                events: vec![(5, 1), (31, 2)],
                dropped: 0,
            },
            Piece {
                frames: 16,
                events: Vec::new(),
                dropped: 0,
            },
        ]
    );
    assert!(
        audio.out_events.as_slice().is_empty(),
        "the last block sent none"
    );
}

/// Records, for each piece, the gain it read and the controllers it was
/// handed.
#[derive(Debug)]
struct Reads {
    set: Arc<ParamSet>,
    gain: ParamId,
    pieces: Vec<(f32, usize)>,
}

impl Processor for Reads {
    type Layout = AnyLayout;

    fn process(&mut self, block: Block<'_, AnyLayout>) -> Status {
        let events = block.midi.packets().len();
        self.pieces.push((self.set.value(self.gain), events));
        Status::Continue
    }
}

/// A gain from 0 to 1 that starts at 0, learn over it, and block state whose
/// processor reads the gain, with learn's audio side in it, in pieces of up
/// to 64 frames.
fn learning() -> (Learn, Active<Reads>, ParamId) {
    let decl = ParamDecl::new("gain", "Gain", Kind::continuous(0.0, 1.0, 0.0, None), 0.0);
    let set = ParamSet::from_decls([decl]).expect("valid");
    let gain = set.id("gain").expect("declared");
    let (learn, learn_audio) = Learn::new(&set, Config::default());
    let reads = Reads {
        set,
        gain,
        pieces: Vec::new(),
    };
    let max_frames = NonZeroUsize::new(64).expect("not 0");
    let mut audio = Active::around(reads, Width::new(0, 0), max_frames, 4, 0);
    audio.learn = Some(learn_audio);
    (learn, audio, gain)
}

/// Returns a MIDI 1.0 control change on group 0, channel 0, at `offset`,
/// upgraded to the MIDI 2.0 that learn reads.
pub(in crate::engine) fn upgraded_cc(offset: u32, controller: u8, value: u8) -> Event {
    let message = Midi1Message::ControlChange { controller, value };
    let sent = Midi1Voice {
        group: 0,
        channel: 0,
        message,
    }
    .event(offset);
    let mut upgraded = EventList::with_capacity(1);
    Upgrade::new().block(&[sent], &mut upgraded);
    *upgraded
        .as_slice()
        .first()
        .expect("one controller upgrades to one")
}

#[test]
fn a_learned_controller_is_written_before_the_processor_reads_in_its_own_block() {
    let (mut learn, mut audio, gain) = learning();
    learn
        .map(gain, Mapping::new(Source::any(Control::Cc(7))))
        .expect("mapped");
    audio.events.push(upgraded_cc(10, 7, 127));
    // SAFETY: a layout of no channels, so no buffer is read or written.
    unsafe { pieces(&mut audio, 32, &[], &[]) };

    assert_eq!(
        audio.processor.pieces,
        [(1.0, 1)],
        "the controller's value, and the controller itself"
    );
}

#[test]
fn a_controller_in_a_later_piece_is_read_by_every_piece_of_its_block() {
    let (mut learn, mut audio, gain) = learning();
    learn
        .map(gain, Mapping::new(Source::any(Control::Cc(7))))
        .expect("mapped");
    audio.events.push(upgraded_cc(100, 7, 127));
    // SAFETY: as above.
    unsafe { pieces(&mut audio, 150, &[], &[]) };

    assert_eq!(
        audio.processor.pieces,
        [(1.0, 0), (1.0, 1), (1.0, 0)],
        "learn ran once over the whole block before the first piece, as the \
         host's automation lands, and the controller still reaches its piece"
    );
}

#[test]
fn a_controller_a_block_brings_while_a_target_is_armed_is_learned_at_the_next_poll() {
    let (mut learn, mut audio, gain) = learning();
    learn.arm(gain, Takes::CONTROLLER).expect("a target");
    audio.events.push(upgraded_cc(0, 74, 64));
    // SAFETY: as above.
    unsafe { pieces(&mut audio, 16, &[], &[]) };

    assert!(
        matches!(learn.poll(), Some(Learned::Mapped { .. })),
        "the block staged it, the poll commits it"
    );
    assert_eq!(
        learn.mapping(gain).map(|mapping| mapping.source.control),
        Some(Control::Cc(74))
    );
}

/// A MIDI 1.0 control change at `offset`, as a host sends it.
fn sent(offset: u32, controller: u8, value: u8) -> Event {
    let message = Midi1Message::ControlChange { controller, value };
    Midi1Voice {
        group: 0,
        channel: 0,
        message,
    }
    .event(offset)
}

#[test]
fn midi_1_an_exporter_hands_over_reaches_the_block_as_midi_2_and_midi_2_as_sent() {
    let mut audio = audio();
    let midi2 = Event::new(9, [0x40B0_0700, 1, 0, 0]);
    fill(&mut audio, 64, |into| {
        into.push(sent(3, 7, 127));
        into.push(midi2);
    });
    let events = audio.events.as_slice();
    assert_eq!(events.len(), 2);
    assert_eq!((events[0].offset, events[0].origin), (3, Origin::Midi1));
    let message = events[0].channel_voice().map(|voice| voice.message);
    let full = ChannelMessage::ControlChange {
        controller: 7,
        value: u32::MAX,
    };
    assert_eq!(message, Some(full));
    assert_eq!(events[1], midi2);
}

#[test]
fn a_high_byte_sent_alone_reaches_its_block_at_the_last_frame_and_the_next_starts_clean() {
    let mut audio = audio();
    fill(&mut audio, 32, |into| {
        into.push(sent(0, 99, 2));
        into.push(sent(0, 98, 44));
        into.push(sent(4, 6, 64));
    });
    let events: Vec<_> = audio
        .events
        .as_slice()
        .iter()
        .map(|event| {
            (
                event.offset,
                event.channel_voice().map(|voice| voice.message),
            )
        })
        .collect();
    let entry = ChannelMessage::AssignableController {
        bank: 2,
        index: 44,
        value: hapto_ump::scale::up_14_to_32(64 << 7),
    };
    assert_eq!(events, [(31, Some(entry))]);
    fill(&mut audio, 32, |_| {});
    assert!(audio.events.as_slice().is_empty(), "nothing held over");
}

#[test]
fn packets_handed_over_out_of_order_reach_the_block_in_time_order_and_at_one_frame_as_they_came() {
    let mut audio = audio();
    let off = Event::new(9, [0x4080_3C00, 0, 0, 0]);
    let on = Event::new(9, [0x4090_3C00, 0xFFFF_0000, 0, 0]);
    // VST3 hands each controller over in its own queue, so here an RPN's data
    // entry comes before its address. At frame 9, a note-off and then a
    // note-on must stay in that order.
    fill(&mut audio, 32, |into| {
        into.push(sent(5, 6, 64));
        into.push(off);
        into.push(sent(2, 101, 0));
        into.push(on);
        into.push(sent(2, 100, 0));
        into.push(sent(5, 38, 0));
    });
    let events: Vec<_> = audio
        .events
        .as_slice()
        .iter()
        .map(|event| (event.offset, event.words))
        .collect();
    let range = ChannelVoice {
        group: 0,
        channel: 0,
        message: ChannelMessage::RegisteredController {
            bank: 0,
            index: 0,
            value: hapto_ump::scale::up_14_to_32(64 << 7),
        },
    };
    assert_eq!(
        events,
        [(5, range.words()), (9, off.words), (9, on.words)],
        "the address first, so the entry has one"
    );
}

#[test]
fn a_packet_pushed_first_goes_before_the_rest_at_its_frame_and_never_to_an_earlier_one() {
    let mut audio = audio();
    fill(&mut audio, 32, |into| {
        into.push(sent(2, 6, 64));
        into.push(sent(1, 7, 100));
        into.push_ranked(sent(2, 101, 0), Rank::Address);
        into.push_ranked(sent(2, 100, 0), Rank::Address);
    });
    let events: Vec<_> = audio
        .events
        .as_slice()
        .iter()
        .map(|event| {
            (
                event.offset,
                event.channel_voice().map(|voice| voice.message),
            )
        })
        .collect();
    let volume = ChannelMessage::ControlChange {
        controller: 7,
        value: hapto_ump::scale::up_7_to_32(100),
    };
    let range = ChannelMessage::RegisteredController {
        bank: 0,
        index: 0,
        value: hapto_ump::scale::up_14_to_32(64 << 7),
    };
    // The data entry at frame 2 finds the address pushed after it, and with
    // no low byte it goes out at the block's last frame. The volume at frame
    // 1 still comes first.
    assert_eq!(events, [(1, Some(volume)), (31, Some(range))]);
}

#[test]
fn a_high_byte_goes_before_a_low_byte_at_its_frame_whatever_order_they_come_in() {
    let mut audio = audio();
    let entry = |value| ChannelMessage::AssignableController {
        bank: 2,
        index: 44,
        value: hapto_ump::scale::up_14_to_32(value),
    };
    // A first move of NRPN 2/44.
    fill(&mut audio, 32, |into| {
        into.push_ranked(sent(0, 99, 2), Rank::Address);
        into.push_ranked(sent(0, 98, 44), Rank::Address);
        into.push_ranked(sent(0, 6, 64), Rank::HighByte);
        into.push(sent(0, 38, 10));
    });
    // The next move lists its low byte's queue first, and the low byte still
    // refines its own high byte, not the one before.
    fill(&mut audio, 32, |into| {
        into.push(sent(5, 38, 3));
        into.push_ranked(sent(5, 6, 65), Rank::HighByte);
    });
    let events: Vec<_> = audio
        .events
        .as_slice()
        .iter()
        .map(|event| {
            (
                event.offset,
                event.channel_voice().map(|voice| voice.message),
            )
        })
        .collect();
    assert_eq!(events, [(5, Some(entry(65 << 7 | 3)))]);
}

/// Sends a note far past the end of every piece it plays.
struct Late;

impl Processor for Late {
    type Layout = AnyLayout;

    fn process(&mut self, mut block: Block<'_, AnyLayout>) -> Status {
        block.midi_out.extend([note(100, 1)]);
        Status::Continue
    }
}

#[test]
fn what_a_piece_sends_past_its_end_goes_at_its_last_frame() {
    let max_frames = NonZeroUsize::new(4).expect("not 0");
    let mut audio = Active::around(Late, Width::new(0, 0), max_frames, 0, 4);
    // SAFETY: a layout of no channels, so no buffer is read or written.
    let status = unsafe { pieces(&mut audio, 10, &[], &[]) };
    assert_eq!(status, Status::Continue);
    let sent: Vec<_> = audio.out_events.as_slice().iter().map(tagged).collect();
    assert_eq!(
        sent,
        [(3, 1), (7, 1), (9, 1)],
        "each in its own piece, in time order, none past the block"
    );
}

#[test]
fn packets_past_the_room_are_dropped_and_counted_before_they_are_put_in_order() {
    // Room for six.
    let mut audio = audio();
    fill(&mut audio, 32, |into| {
        for frame in (0..8).rev() {
            into.push(Event::new(frame, [0x4090_3C00, 0xFFFF_0000, 0, 0]));
        }
    });
    let frames: Vec<u32> = audio
        .events
        .as_slice()
        .iter()
        .map(|event| event.offset)
        .collect();
    assert_eq!(frames, [2, 3, 4, 5, 6, 7], "the first six handed over");
    assert_eq!(audio.events.dropped(), 2);
}
