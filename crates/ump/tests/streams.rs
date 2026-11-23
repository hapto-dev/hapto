//! Whole MIDI 1.0 streams cut into blocks anywhere, through the upgrade and
//! back through the downgrade: what the downgrade sends upgrades to the same
//! messages, and a MIDI 1.0 receiver of it ends each block where the sender
//! put it.

use std::collections::BTreeMap;

use hapto_ump::{
    ChannelMessage, Downgrade, Event, EventList, Midi1Message, Midi1Voice, Origin, Upgrade,
};
use proptest::prelude::*;
use proptest::test_runner::FileFailurePersistence;

/// The frame a block's held high bytes go out at.
const LAST_FRAME: u32 = 63;

/// Each block's packets in turn.
type Blocks = Vec<Vec<Event>>;

/// `sent` upgraded a block at a time by one upgrade, as an exporter does.
fn upgrade(sent: &[Vec<Event>]) -> Blocks {
    let mut upgrade = Upgrade::new();
    sent.iter()
        .map(|block| {
            // Room for every message and a high byte held on every group
            // and channel.
            let mut out = EventList::with_capacity(block.len() + 256);
            upgrade.block(block, LAST_FRAME, &mut out);
            assert_eq!(out.dropped(), 0);
            out.as_slice().to_vec()
        })
        .collect()
}

/// `upgraded` downgraded a block at a time, losing nothing.
fn downgrade(upgraded: &[Vec<Event>]) -> Blocks {
    let mut downgrade = Downgrade::new();
    let back = upgraded
        .iter()
        .map(|block| {
            // A program change with its bank is three messages, a
            // controller four.
            let mut out = EventList::with_capacity(block.len() * 4);
            downgrade.block(block, &mut out);
            assert_eq!(out.dropped(), 0);
            out.as_slice().to_vec()
        })
        .collect();
    assert_eq!(downgrade.lost(), 0, "MIDI 1.0 carries all an upgrade makes");
    back
}

/// Each block's messages at their frames, their origins aside.
fn messages(blocks: &[Vec<Event>]) -> Vec<Vec<(u32, u8, u8, ChannelMessage)>> {
    blocks
        .iter()
        .map(|block| {
            block
                .iter()
                .map(|event| {
                    let voice = event.channel_voice().expect("MIDI 2.0");
                    (event.offset, voice.group, voice.channel, voice.message)
                })
                .collect()
        })
        .collect()
}

/// Blocks of `sent`, each message on its group and channel and after a cut
/// when its flag says so, at frames that rise through the block.
fn blocks(sent: Vec<(bool, u8, u8, Midi1Message)>) -> Blocks {
    let mut blocks: Blocks = vec![Vec::new()];
    for (cut, group, channel, message) in sent {
        if cut {
            blocks.push(Vec::new());
        }
        let block = blocks.last_mut().expect("one block at least");
        let frame = u32::try_from(block.len()).map_or(LAST_FRAME, |at| at.min(LAST_FRAME));
        let voice = Midi1Voice {
            group,
            channel,
            message,
        };
        block.push(voice.event(frame));
    }
    blocks
}

/// Any MIDI 1.0 channel voice message, with the controllers that hold state
/// often.
fn midi1_message() -> impl Strategy<Value = Midi1Message> {
    let byte = || prop_oneof![Just(0_u8), Just(127_u8), 0_u8..=127];
    let controller = prop::sample::select(vec![
        0_u8, 6, 6, 7, 32, 38, 38, 64, 96, 97, 98, 98, 99, 99, 100, 101, 121,
    ]);
    prop_oneof![
        6 => (controller, byte())
            .prop_map(|(controller, value)| Midi1Message::ControlChange { controller, value }),
        1 => (byte(), byte()).prop_map(|(note, velocity)| Midi1Message::NoteOn { note, velocity }),
        1 => (byte(), byte()).prop_map(|(note, velocity)| Midi1Message::NoteOff { note, velocity }),
        1 => (byte(), byte()).prop_map(|(note, value)| Midi1Message::PolyPressure { note, value }),
        1 => byte().prop_map(|value| Midi1Message::ChannelPressure { value }),
        1 => (0_u16..=0x3FFF).prop_map(|value| Midi1Message::PitchBend { value }),
        1 => byte().prop_map(|program| Midi1Message::ProgramChange { program }),
    ]
}

/// A parameter number as a MIDI 1.0 receiver selects it: registered or
/// not, and its 14 bits.
type Number = (bool, u16);

/// A MIDI 1.0 receiver's parameter numbers on one channel, as the MIDI 1.0
/// specification and RP-015 read them.
#[derive(Debug)]
struct Receiver {
    nrpn: [u8; 2],
    rpn: [u8; 2],
    registered: Option<bool>,
    values: BTreeMap<Number, u16>,
}

impl Receiver {
    fn new() -> Self {
        Receiver {
            nrpn: [127, 127],
            rpn: [127, 127],
            registered: None,
            values: BTreeMap::new(),
        }
    }

    /// The selected number, none while the null number is selected.
    fn number(&self) -> Option<Number> {
        let registered = self.registered?;
        let [high, low] = if registered { self.rpn } else { self.nrpn };
        ((high, low) != (127, 127)).then_some((registered, u16::from(high) << 7 | u16::from(low)))
    }

    fn hear(&mut self, controller: u8, value: u8) {
        match controller {
            99 | 98 => {
                self.nrpn[usize::from(controller == 98)] = value;
                self.registered = Some(false);
            }
            101 | 100 => {
                self.rpn[usize::from(controller == 100)] = value;
                self.registered = Some(true);
            }
            // A high byte sets the low byte to 0.
            6 => {
                if let Some(number) = self.number() {
                    self.values.insert(number, u16::from(value) << 7);
                }
            }
            38 => {
                if let Some(number) = self.number() {
                    let entry = self.values.entry(number).or_insert(0);
                    *entry = *entry & !0x7F | u16::from(value);
                }
            }
            // Reset All Controllers selects the null number.
            121 => {
                *self = Receiver {
                    values: std::mem::take(&mut self.values),
                    ..Receiver::new()
                }
            }
            _ => {}
        }
    }

    fn hear_block(&mut self, block: &[Event]) {
        for event in block {
            let voice = event.midi1_voice().expect("MIDI 1.0");
            if let Midi1Message::ControlChange { controller, value } = voice.message {
                self.hear(controller, value);
            }
        }
    }
}

/// One step of a sender of RPNs and NRPNs.
#[derive(Debug, Clone)]
enum Step {
    /// A whole address, in either byte order.
    Address {
        registered: bool,
        number: [u8; 2],
        low_first: bool,
    },
    High(u8),
    Low(u8),
    Other(u8),
    Reset,
    Cut,
}

fn step() -> impl Strategy<Value = Step> {
    let byte = || prop::sample::select(vec![0_u8, 1, 64, 127]);
    // Few numbers, so they repeat, and 127 with 127 the null number.
    let part = || prop::sample::select(vec![0_u8, 1, 127]);
    prop_oneof![
        2 => (any::<bool>(), part(), part(), any::<bool>()).prop_map(
            |(registered, high, low, low_first)| Step::Address {
                registered,
                number: [high, low],
                low_first,
            }
        ),
        3 => byte().prop_map(Step::High),
        3 => byte().prop_map(Step::Low),
        1 => byte().prop_map(Step::Other),
        1 => Just(Step::Reset),
        2 => Just(Step::Cut),
    ]
}

/// The blocks a sender sends for `steps`, as MIDI 1.0 control changes on
/// group 0 and channel 0.
///
/// The sender sends a number's high byte before its low byte: a low byte
/// goes out only under the number the newest high byte was sent under, as
/// most senders send them. The upgrade departs from MIDI 1.0 for the rest,
/// where a low byte refines the receiver's own high byte for its number,
/// which no translator holds.
fn sender(steps: Vec<Step>) -> Blocks {
    let mut sent = Vec::new();
    let mut mirror = Receiver::new();
    // The number the newest high byte since Reset All Controllers was sent
    // under, if one was.
    let mut high_under = None;
    let mut cut = false;
    for step in steps {
        let mut out = |controller: u8, value: u8, mirror: &mut Receiver| {
            mirror.hear(controller, value);
            let message = Midi1Message::ControlChange { controller, value };
            sent.push((std::mem::take(&mut cut), 0, 0, message));
        };
        match step {
            Step::Address {
                registered,
                number: [high, low],
                low_first,
            } => {
                let (high_byte, low_byte) = if registered { (101, 100) } else { (99, 98) };
                if low_first {
                    out(low_byte, low, &mut mirror);
                    out(high_byte, high, &mut mirror);
                } else {
                    out(high_byte, high, &mut mirror);
                    out(low_byte, low, &mut mirror);
                }
            }
            Step::High(value) => {
                high_under = Some(mirror.number());
                out(6, value, &mut mirror);
            }
            Step::Low(value) => {
                let number = mirror.number();
                if number.is_none() || high_under == Some(number) {
                    out(38, value, &mut mirror);
                }
            }
            Step::Other(value) => out(7, value, &mut mirror),
            Step::Reset => {
                high_under = None;
                out(121, 0, &mut mirror);
            }
            Step::Cut => cut = true,
        }
    }
    blocks(sent)
}

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
            "tests/streams.proptest-regressions",
        ))),
        cases: 1024,
        ..ProptestConfig::default()
    })]

    /// Upgraded, downgraded and upgraded again, any stream gives the same
    /// messages at the same frames: the downgrade sends nothing the upgrade
    /// reads another way, however the blocks cut the data entry it holds.
    #[test]
    fn what_the_downgrade_sends_upgrades_to_the_same_messages(
        sent in proptest::collection::vec(
            (prop::bool::weighted(0.2), 0_u8..2, 0_u8..2, midi1_message()),
            0..64,
        ),
    ) {
        let upgraded = upgrade(&blocks(sent));
        for event in upgraded.iter().flatten() {
            prop_assert!(matches!(event.origin, Origin::Midi1 | Origin::Midi1HighByte));
        }
        let again = upgrade(&downgrade(&upgraded));
        prop_assert_eq!(messages(&again), messages(&upgraded));
    }

    /// A MIDI 1.0 receiver of the downgrade holds, at the end of every block,
    /// the values a receiver of the sender holds: the upgrade sends every
    /// data entry it holds by the block's end, and sends each where MIDI 1.0
    /// puts it.
    #[test]
    fn a_receiver_of_the_downgrade_ends_each_block_where_the_sender_put_it(
        steps in proptest::collection::vec(step(), 0..48),
    ) {
        let sent = sender(steps);
        let back = downgrade(&upgrade(&sent));
        let (mut direct, mut through) = (Receiver::new(), Receiver::new());
        for (sent, back) in sent.iter().zip(&back) {
            direct.hear_block(sent);
            through.hear_block(back);
            prop_assert_eq!(&through.values, &direct.values, "{:?}", sent);
        }
    }
}
