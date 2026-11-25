//! Converts packets to learnable messages.
//!
//! Learn reads only MIDI 2.0. An exporter upgrades MIDI 1.0 on the way in
//! with [`Upgrade`](hapto_ump::Upgrade), which assembles an NRPN's four
//! controllers into one assignable controller. A value upgraded from MIDI 1.0
//! is narrowed back to its original width. Parsing keeps no state, because
//! `Upgrade` holds it.

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

use hapto_ump::controller::is_parameter_number;
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

/// A message's value at its original width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Value {
    /// A MIDI 1.0 controller value or velocity.
    Seven(u8),
    /// A MIDI 1.0 NRPN's data entry high byte, and its low byte if one came.
    Nrpn { high: u8, low: Option<u8> },
    /// A MIDI 2.0 velocity.
    Sixteen(u16),
    /// A MIDI 2.0 controller value.
    ThirtyTwo(u32),
}

impl Value {
    /// The value as 0..1 for a binding, [`coarse`](crate::Mapping::coarse) or
    /// not; none for a binding that is not coarse and a high byte sent alone,
    /// whose low byte it waits for.
    pub(crate) fn normalized(self, coarse: bool) -> Option<f64> {
        Some(match self {
            Value::Seven(value) => scale::seven(value),
            Value::Nrpn { high, .. } if coarse => scale::seven(high),
            Value::Nrpn {
                high,
                low: Some(low),
            } => scale::fourteen(u16::from(high) << 7 | u16::from(low)),
            Value::Nrpn { low: None, .. } => return None,
            Value::Sixteen(value) => scale::sixteen(value),
            Value::ThirtyTwo(value) => scale::thirty_two(value),
        })
    }

    /// Returns the 14-bit value of an assignable controller upgraded from an
    /// NRPN, with no low byte if `Upgrade` sent the high byte alone.
    fn nrpn(value: u32, origin: Origin) -> Value {
        let entry = down_32_to_14(value);
        Value::Nrpn {
            high: (entry >> 7) as u8,
            low: (origin != Origin::Midi1HighByte).then_some((entry & 0x7F) as u8),
        }
    }
}

/// Returns the learnable message in `event`, if it carries one.
pub(crate) fn parse(event: &Event) -> Option<Message> {
    let ChannelVoice {
        group,
        channel,
        message,
    } = event.channel_voice()?;
    let midi1 = matches!(event.origin, Origin::Midi1 | Origin::Midi1HighByte);
    let (control, value) = match message {
        // MIDI 2.0 sends parameter numbers and their relative changes as
        // separate messages, so the MIDI 1.0 controllers that select, enter
        // and step them mean nothing here. That includes the increment and
        // decrement that `Upgrade` passes on as controllers.
        ChannelMessage::ControlChange { controller, value } if !is_parameter_number(controller) => {
            let value = if midi1 {
                Value::Seven(down_32_to_7(value))
            } else {
                Value::ThirtyTwo(value)
            };
            (Control::Cc(controller), value)
        }
        // In MIDI 2.0, a note-on at velocity 0 is still a note-on, so it
        // writes 0.0. `Upgrade` turns a MIDI 1.0 one into a note-off, which
        // writes nothing.
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
                Value::nrpn(value, event.origin)
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

    /// Returns what the last control change in `history` means under MIDI
    /// 1.0, except where Hapto departs from it on purpose.
    ///
    /// `history` is one channel's control changes in order. The model rereads
    /// all of it for each message instead of keeping state, so it shares no
    /// structure with `Upgrade` or the parser. MIDI 1.0 has these rules:
    ///
    /// - Controllers 99 and 98 each latch one byte of the NRPN number, in
    ///   either order, and 101 and 100 latch the RPN number.
    /// - Data entry applies to the number that the newest of the four
    ///   selected. An address of 127 and 127 selects no number.
    /// - Reset All Controllers (121) sets the number to null, as RP-015
    ///   recommends.
    /// - A data entry high byte (6) sets the low byte to zero, and a low byte
    ///   (38) refines the held high byte.
    ///
    /// Hapto's deviations, which `Upgrade` or the parser documents:
    ///
    /// 1. A 38 refines the newest 6 only when that 6 was sent under the same
    ///    number, however often the address was sent between. Otherwise it
    ///    waits, and the next 6 carries it when no address byte comes
    ///    between and the 6 is under its number. MIDI 1.0 would refine the
    ///    receiver's own high byte for that number, which learn does not
    ///    hold.
    /// 2. Increment and decrement (96, 97) drive nothing, since learn holds no
    ///    value to step.
    /// 3. Reset All Controllers also forgets both latched bytes, so data entry
    ///    after it waits for a whole address. RP-015's null leaves both bytes
    ///    at 127, so a lone 98 after it would select a number.
    fn model(history: &[(u8, u8)]) -> Option<(Control, Value)> {
        let (&(controller, value), earlier) = history.split_last()?;
        if !matches!(controller, 6 | 38 | 96 | 97 | 98 | 99 | 100 | 101) {
            return Some((Control::Cc(controller), Value::Seven(value)));
        }
        // Address messages only select, and deviation 2 drops the steps.
        if !matches!(controller, 6 | 38) {
            return None;
        }
        let earlier = since_reset(earlier);
        // Learn reads only NRPNs.
        let Some(Number::Nrpn(nrpn)) = number(earlier) else {
            return None;
        };
        let newest_high = earlier.iter().rposition(|&(c, _)| c == 6);
        // The number that the data entry byte at `at` was sent under.
        let under = |at: usize| number(&earlier[..at]);
        if controller == 38 {
            // Deviation 1: another number's high byte waits.
            let at = newest_high?;
            if under(at) != Some(Number::Nrpn(nrpn)) {
                return None;
            }
            let high = earlier[at].1;
            return Some((
                Control::Nrpn(nrpn),
                Value::Nrpn {
                    high,
                    low: Some(value),
                },
            ));
        }
        // Zero, as MIDI 1.0 says, unless deviation 1 holds a 38 for it: the
        // newest 38 since the last 6 and the last address byte that came
        // before its own number's high byte.
        let held = newest_high.and_then(under);
        let newest_address = earlier.iter().rposition(|&(c, _)| matches!(c, 98..=101));
        let start = newest_high.max(newest_address).map_or(0, |at| at + 1);
        let early = (start..earlier.len())
            .rev()
            .filter(|&at| earlier[at].0 == 38)
            .map(|at| (under(at), earlier[at].1))
            .find(|&(sent, _)| sent.is_some() && sent != held);
        let low = early
            .filter(|&(sent, _)| sent == Some(Number::Nrpn(nrpn)))
            .map(|(_, low)| low);
        Some((Control::Nrpn(nrpn), Value::Nrpn { high: value, low }))
    }

    /// A parameter number, as MIDI 1.0 selects it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Number {
        Nrpn(u16),
        Rpn(u16),
    }

    /// What follows the newest Reset All Controllers in `history`, which
    /// forgets the address (deviation 3).
    fn since_reset(history: &[(u8, u8)]) -> &[(u8, u8)] {
        match history.iter().rposition(|&(c, _)| c == 121) {
            Some(reset) => &history[reset + 1..],
            None => history,
        }
    }

    /// Returns the number that the address bytes in `history` select, if
    /// any.
    fn number(history: &[(u8, u8)]) -> Option<Number> {
        let history = since_reset(history);
        let newest = |controller: u8, within: &[(u8, u8)]| {
            within
                .iter()
                .rev()
                .find(|&&(c, _)| c == controller)
                .map(|&(_, v)| v)
        };
        let address = history.iter().rposition(|&(c, _)| matches!(c, 98..=101))?;
        let (high_byte, low_byte) = if matches!(history[address].0, 98 | 99) {
            (99, 98)
        } else {
            (101, 100)
        };
        let high = newest(high_byte, history)?;
        let low = newest(low_byte, history)?;
        if (high, low) == (127, 127) {
            return None;
        }
        let number = u16::from(high) << 7 | u16::from(low);
        Some(if high_byte == 99 {
            Number::Nrpn(number)
        } else {
            Number::Rpn(number)
        })
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
        /// arrives in a block of its own.
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
                upgrade.block(&[event], &mut upgraded);
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
