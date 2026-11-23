//! From packets to learnable messages.
//!
//! Learn reads MIDI 2.0 alone. An exporter upgrades MIDI 1.0 on the way in
//! with [`Upgrade`](hapto_ump::Upgrade), which assembles an NRPN from its
//! four controllers into one assignable controller, and a packet upgraded
//! from MIDI 1.0 narrows back to the width it was sent at. So a MIDI 1.0
//! controller reads as MIDI 1.0 sent it, and a MIDI 2.0 one at its own
//! width. Reading keeps no state: the upgrade holds MIDI 1.0's.

// Audio-path code: a panic here ends a host, so indexing, unwrap, expect,
// panic and unchecked arithmetic are refused, and a site that is provably
// safe says why in an `#[expect]`.
#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects
    )
)]

use hapto_ump::scale::{down_16_to_7, down_32_to_7, down_32_to_14};
use hapto_ump::{ChannelMessage, ChannelVoice, Event, Origin};

use crate::scale;
use crate::source::Control;

/// A learnable message.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Message {
    pub(crate) control: Control,
    pub(crate) group: u8,
    pub(crate) channel: u8,
    pub(crate) value: Value,
}

/// A message's value at the width it was sent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Value {
    /// A MIDI 1.0 controller value or velocity.
    Seven(u8),
    /// A MIDI 1.0 NRPN's data entry: the high byte, and the low byte unless
    /// it is 0.
    ///
    /// The upgrade sends a high byte that no low byte followed as if the low
    /// byte were 0, as the specification says, so a low byte of 0 reads as
    /// none. The value is the same either way.
    Nrpn { high: u8, low: Option<u8> },
    /// A MIDI 2.0 velocity.
    Sixteen(u16),
    /// A MIDI 2.0 controller value.
    ThirtyTwo(u32),
}

impl Value {
    /// The value as 0..1 for a binding, [`coarse`](crate::Mapping::coarse) or
    /// not.
    pub(crate) fn normalized(self, coarse: bool) -> f64 {
        match self {
            Value::Seven(value) => scale::seven(value),
            Value::Nrpn { high, .. } if coarse => scale::seven(high),
            Value::Nrpn { high, low } => {
                scale::fourteen(u16::from(high) << 7 | u16::from(low.unwrap_or(0)))
            }
            Value::Sixteen(value) => scale::sixteen(value),
            Value::ThirtyTwo(value) => scale::thirty_two(value),
        }
    }

    /// An assignable controller's value upgraded from an NRPN, back at the
    /// 14 bits it was sent at.
    fn nrpn(value: u32) -> Value {
        let entry = down_32_to_14(value);
        let low = (entry & 0x7F) as u8;
        Value::Nrpn {
            high: (entry >> 7) as u8,
            low: (low != 0).then_some(low),
        }
    }
}

/// The learnable message in `event`, if it carries one.
pub(crate) fn parse(event: &Event) -> Option<Message> {
    let ChannelVoice {
        group,
        channel,
        message,
    } = event.channel_voice()?;
    let midi1 = event.origin == Origin::Midi1;
    let (control, value) = match message {
        // MIDI 2.0 carries parameter numbers and their relative changes as
        // messages of their own, so the controllers MIDI 1.0 selects, enters
        // and steps them with mean nothing here. The upgrade sends MIDI 1.0's
        // increment and decrement on as controllers, which learn ignores too.
        ChannelMessage::ControlChange { controller, value }
            if !Control::PARSER_OWNED.contains(&controller) =>
        {
            let value = if midi1 {
                Value::Seven(down_32_to_7(value))
            } else {
                Value::ThirtyTwo(value)
            };
            (Control::Cc(controller), value)
        }
        // A MIDI 2.0 note-on at velocity 0 is a note-on, as its
        // specification says, so it writes 0.0. The upgrade sends a MIDI 1.0
        // one as the note-off it means, and a note-off writes nothing.
        ChannelMessage::NoteOn { note, velocity, .. } => {
            let value = if midi1 {
                Value::Seven(down_16_to_7(velocity))
            } else {
                Value::Sixteen(velocity)
            };
            (Control::Note(note), value)
        }
        ChannelMessage::AssignableController { bank, index, value } => {
            let value = if midi1 {
                Value::nrpn(value)
            } else {
                Value::ThirtyTwo(value)
            };
            (
                Control::Nrpn(u16::from(bank) << 7 | u16::from(index)),
                value,
            )
        }
        _ => return None,
    };
    Some(Message {
        control,
        group,
        channel,
        value,
    })
}

#[cfg(test)]
mod tests {
    use hapto_ump::{EventList, Midi1Message, Midi1Voice, Upgrade};
    use proptest::prelude::*;
    use proptest::test_runner::FileFailurePersistence;

    use super::*;

    /// What the last of `history`, one channel's control changes in order,
    /// means: MIDI 1.0's reading, except where Hapto departs from it on
    /// purpose.
    ///
    /// The reading goes over the whole history again for each message instead
    /// of keeping state, so it shares no structure with the upgrade or the
    /// parser.
    ///
    /// MIDI 1.0: controllers 99 and 98 each latch one byte of the NRPN
    /// number, in either order, and 101 and 100 the RPN's. Data entry is for
    /// the number the newest of the four selected, and 127 and 127 select
    /// nothing. Reset All Controllers (121) sets the number to null, as RP-015
    /// recommends. A data entry high byte (6) sets the low byte to zero, and a
    /// low byte (38) refines the high byte held.
    ///
    /// Hapto's deviations, each the upgrade's or the parser's documented
    /// choice:
    ///
    /// 1. A 99 that begins an address forgets the latched 98, which belonged
    ///    to an address already used ([`begins_an_address`]).
    /// 2. A 38 before any 6 since the address waits, and the first 6 carries
    ///    it. MIDI 1.0 would refine the receiver's own high byte, which learn
    ///    does not hold.
    /// 3. Increment and decrement (96, 97) drive nothing, since learn holds no
    ///    value to step.
    /// 4. Reset All Controllers also forgets both latched bytes, so data entry
    ///    after it waits for a whole address. RP-015's null leaves both bytes
    ///    at 127, so a lone 98 after it would select a number.
    /// 5. A low byte of 0 reads as none, since the upgrade sends a high byte
    ///    alone as if its low byte were 0.
    fn model(history: &[(u8, u8)]) -> Option<(Control, Value)> {
        let (&(controller, value), earlier) = history.split_last()?;
        if !matches!(controller, 6 | 38 | 96 | 97 | 98 | 99 | 100 | 101) {
            return Some((Control::Cc(controller), Value::Seven(value)));
        }
        // Address messages only select, and deviation 3 drops the steps.
        if !matches!(controller, 6 | 38) {
            return None;
        }
        let earlier = match earlier.iter().rposition(|&(c, _)| c == 121) {
            Some(reset) => &earlier[reset + 1..],
            None => earlier,
        };
        let newest = |controller: u8, within: &[(u8, u8)]| {
            within
                .iter()
                .rev()
                .find(|&&(c, _)| c == controller)
                .map(|&(_, v)| v)
        };
        let address = earlier.iter().rposition(|&(c, _)| matches!(c, 98..=101))?;
        if !matches!(earlier[address].0, 98 | 99) {
            return None;
        }
        let high = newest(99, earlier)?;
        let since_high = &earlier[earlier.iter().rposition(|&(c, _)| c == 99)? + 1..];
        let forgotten = begins_an_address(earlier) && newest(98, since_high).is_none();
        let low = newest(98, earlier).filter(|_| !forgotten)?;
        if (high, low) == (127, 127) {
            return None;
        }
        let number = u16::from(high) << 7 | u16::from(low);
        let since = &earlier[address + 1..];
        if controller == 6 {
            // Zero, as MIDI 1.0 says, unless deviation 2 holds a 38 for it.
            let first_high = since.iter().all(|&(c, _)| c != 6);
            let low = newest(38, since).filter(|&low| first_high && low != 0);
            return Some((Control::Nrpn(number), Value::Nrpn { high: value, low }));
        }
        // A 38 with no 6 since the address waits for one (deviation 2).
        let high = newest(6, since)?;
        Some((
            Control::Nrpn(number),
            Value::Nrpn {
                high,
                low: (value != 0).then_some(value),
            },
        ))
    }

    /// Whether the newest 99 in `history` begins an address, rather than
    /// completing one a 98 began.
    ///
    /// Address bytes pair off two by two from the start of a run of them, and
    /// the first of a pair begins an address. A run ends at any other
    /// parameter-number message and at a byte that repeats the one before it,
    /// so within a run the bytes alternate.
    fn begins_an_address(history: &[(u8, u8)]) -> bool {
        let Some(at) = history.iter().rposition(|&(c, _)| c == 99) else {
            return true;
        };
        let mut before_it = 0;
        let mut later = 99;
        for &(c, _) in history[..at].iter().rev() {
            match c {
                98 | 99 if c != later => {
                    before_it += 1;
                    later = c;
                }
                98 | 99 | 6 | 38 | 96 | 97 | 100 | 101 => break,
                // Other controllers leave the address alone.
                _ => {}
            }
        }
        before_it % 2 == 0
    }

    fn control_change() -> impl Strategy<Value = (u8, u8, u8, u8)> {
        (
            0_u8..2,
            0_u8..2,
            prop::sample::select(vec![
                6_u8, 38, 98, 99, 6, 38, 98, 99, 98, 99, 96, 97, 100, 101, 121, 7,
            ]),
            // 127 often, so the null number comes up.
            prop::sample::select(vec![0_u8, 1, 5, 64, 127, 127, 127]),
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            failure_persistence: Some(Box::new(FileFailurePersistence::Direct(
                "tests/parse.proptest-regressions",
            ))),
            cases: 1024,
            ..ProptestConfig::default()
        })]

        /// Whatever order the controllers arrive in, on whichever channels,
        /// upgraded and then parsed they mean what the model says. Each
        /// arrives in a block of its own, so the end of the block sends a
        /// high byte the upgrade holds.
        #[test]
        fn upgraded_midi_1_parses_as_a_model_of_midi_1_0_reads_it(
            changes in proptest::collection::vec(control_change(), 0..48),
        ) {
            let mut upgrade = Upgrade::new();
            let mut upgraded = EventList::with_capacity(4);
            let mut histories = std::collections::HashMap::<(u8, u8), Vec<(u8, u8)>>::new();
            for (group, channel, controller, value) in changes {
                let history = histories.entry((group, channel)).or_default();
                history.push((controller, value));
                let message = Midi1Message::ControlChange { controller, value };
                let event = Midi1Voice { group, channel, message }.event(0);
                upgraded.clear();
                upgrade.block(&[event], 0, &mut upgraded);
                let parsed: Vec<Message> = upgraded.as_slice().iter().filter_map(parse).collect();
                let expected: Vec<Message> = model(history)
                    .map(|(control, value)| Message {
                        control,
                        group,
                        channel,
                        value,
                    })
                    .into_iter()
                    .collect();
                prop_assert_eq!(parsed, expected, "{:?}", history);
            }
        }
    }
}
