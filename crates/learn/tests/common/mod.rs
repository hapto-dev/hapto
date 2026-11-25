//! Helpers for learn's tests and bench, which build a parameter set, test
//! messages and a `Rig` that runs MIDI through learn as an exporter does.
#![allow(dead_code, reason = "each test binary uses only some of it")]

use std::sync::Arc;

use hapto_learn::{Config, Learn, LearnAudio, Mapping, Source};
use hapto_params::{Kind, ParamDecl, ParamId, ParamSet, ParamSetBuilder};
use hapto_ump::{
    ChannelMessage, ChannelVoice, Event, EventList, Midi1Message, Midi1Voice, Upgrade,
};

/// Returns `count` parameters, `p0` onwards, each from 0 to 1 at `default`.
pub(crate) fn set_of(count: usize, default: f32) -> Arc<ParamSet> {
    let mut builder = ParamSetBuilder::new();
    for index in 0..count {
        let key: &'static str = Box::leak(format!("p{index}").into_boxed_str());
        builder
            .add(ParamDecl::new(
                key,
                key,
                Kind::continuous(0.0, 1.0, 0.0, None),
                default,
            ))
            .expect("valid");
    }
    builder.build()
}

/// Learn over a parameter set, with the `Upgrade` that an exporter runs
/// before it.
pub(crate) struct Rig {
    pub(crate) set: Arc<ParamSet>,
    pub(crate) learn: Learn,
    pub(crate) audio: LearnAudio,
    pub(crate) upgrade: Upgrade,
    pub(crate) ids: Vec<ParamId>,
}

impl Rig {
    /// Returns a rig with `count` parameters at `default`.
    pub(crate) fn new(count: usize, default: f32, config: Config) -> Rig {
        let set = set_of(count, default);
        let (learn, audio) = Learn::new(&set, config);
        let ids = set.ids().collect();
        Rig {
            set,
            learn,
            audio,
            upgrade: Upgrade::new(),
            ids,
        }
    }

    /// Runs `sent` through learn as one block, upgraded as an exporter does.
    pub(crate) fn block(&mut self, sent: &[Event]) {
        let events = upgraded(&mut self.upgrade, sent);
        self.audio.process(&events);
    }

    /// Returns the value of parameter `index`.
    pub(crate) fn value(&self, index: usize) -> f32 {
        self.set.value(self.ids[index])
    }

    /// Maps parameter `index` to `source`.
    pub(crate) fn map(&mut self, index: usize, source: Source) {
        self.learn
            .map(self.ids[index], Mapping::new(source))
            .expect("a legal mapping");
    }
}

/// Upgrades `sent` as one block, as an exporter does, with each event at the
/// next frame.
pub(crate) fn upgraded(upgrade: &mut Upgrade, sent: &[Event]) -> EventList {
    let sent: Vec<Event> = sent
        .iter()
        .zip(0..)
        .map(|(event, offset)| Event { offset, ..*event })
        .collect();
    let mut events = EventList::with_capacity(sent.len() + 256);
    upgrade.block(&sent, sent.len() as u32, &mut events);
    events
}

/// Returns a MIDI 1.0 message as a MIDI 1.0 sender sends it.
pub(crate) fn on(group: u8, channel: u8, message: Midi1Message) -> Event {
    Midi1Voice {
        group,
        channel,
        message,
    }
    .event(0)
}

/// Returns a MIDI 2.0 message, which the upgrade passes on unchanged.
pub(crate) fn midi2(group: u8, channel: u8, message: ChannelMessage) -> Event {
    ChannelVoice {
        group,
        channel,
        message,
    }
    .event(0)
}

/// Returns a MIDI 1.0 control change on group 0, channel 0.
pub(crate) fn cc(controller: u8, value: u8) -> Event {
    on(0, 0, Midi1Message::ControlChange { controller, value })
}
