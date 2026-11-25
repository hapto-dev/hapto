//! The plugin-level tests' helper for a MIDI 1.0 controller as learn reads it.
#![allow(dead_code, reason = "each test binary uses only some of it")]

use hapto_ump::{EventList, Midi1Message, Midi1Voice, Upgrade};

/// Returns a MIDI 1.0 control change on group 0, channel 0, at frame 0,
/// upgraded to MIDI 2.0.
pub(crate) fn upgraded_cc(controller: u8, value: u8) -> EventList {
    let message = Midi1Message::ControlChange { controller, value };
    let sent = Midi1Voice {
        group: 0,
        channel: 0,
        message,
    }
    .event(0);
    let mut events = EventList::with_capacity(1);
    Upgrade::new().block(&[sent], &mut events);
    events
}
