//! What every shell tells its host of state the host does not list: once,
//! at the tick after the plugin or its user changed it, and never for a
//! load, a learned controller's write or a listed parameter.

use std::cell::RefCell;
use std::rc::Rc;

use hapto_learn::{Config, Control, LearnAudio, Mapping, Source};
use hapto_params::{Float, FloatDecl, Value};
use hapto_session::{ApplyError, Participant, Registration, Section};
use hapto_ump::{EventList, Midi1Message, Midi1Voice, Upgrade};
use hapto_undo::{Bound, History};
use serde_json::Value as Json;

use super::{Adder, Gain, Heard, Probe, Shell};
use crate::{Driver, Parts, Plugin, PluginInfo, ProcessSetup};

/// Told once of unlisted state, and of nothing else.
const ONCE: Heard = Heard { state: 1, other: 0 };
/// Told nothing.
const NOTHING: Heard = Heard { state: 0, other: 0 };

/// What [`Unlisted`] hands the test, which runs it on the test's own thread
/// and reaches in between ticks, as an editor's code would.
struct Kit {
    parts: Rc<RefCell<Parts>>,
    /// The parameter the host lists.
    gain: Float,
    /// A parameter made at run time, saved under `drive`.
    drive: Float,
    /// A value saved under `label`, which undo records.
    label: Value<String>,
    history: History,
    /// Learn's audio side, which a block would feed.
    learn: LearnAudio,
    /// A participant of the plugin's own, saved under `tab`.
    tab: Rc<RefCell<Tab>>,
}

/// A participant a plugin registers itself: the tab a view shows, which
/// reports its own change from inside its setter.
struct Tab {
    index: u64,
    registration: Option<Registration>,
}

impl Tab {
    /// Sets the tab and reports it, under its owner's borrow, so the session
    /// captures it at the next tick.
    fn set(&mut self, index: u64) {
        self.index = index;
        if let Some(registration) = &self.registration {
            registration.changed();
        }
    }
}

impl Participant for Tab {
    fn capture(&self) -> Section {
        Section::from(self.index)
    }

    fn apply(&mut self, section: Option<&Section>) -> Result<(), ApplyError> {
        self.index = section.and_then(Section::as_u64).unwrap_or(0);
        Ok(())
    }
}

thread_local! {
    static KIT: RefCell<Option<Kit>> = const { RefCell::new(None) };
}

fn kit<R>(f: impl FnOnce(&mut Kit) -> R) -> R {
    KIT.with(|kit| f(kit.borrow_mut().as_mut().expect("a plugin was made")))
}

/// Keeps its parts, with every kind of state a host does not list: a saved
/// value, a parameter made at run time, and learn's mappings of both kinds
/// of parameter.
struct Unlisted {
    _parts: Rc<RefCell<Parts>>,
}

impl Plugin for Unlisted {
    const INFO: PluginInfo = PluginInfo {
        midi_input: true,
        ..Probe::INFO
    };
    type Params = Gain;
    type Processor = Adder;

    fn new(params: Gain, mut parts: Parts) -> Self {
        let history = parts.undo(Bound::default());
        parts.midi_learn(Config::default());
        let label = parts
            .saved_value("label", String::from("Untitled"))
            .expect("a free key");
        history.enroll(&label, "Label").expect("a new value");
        let drive = parts.add("", FloatDecl::new("drive", "Drive", 0.0..=1.0, 0.0));
        // As an activation would take it; this plugin is never activated.
        let learn = Driver::of(&parts).learn_audio().expect("learn is on");
        let tab = Rc::new(RefCell::new(Tab {
            index: 0,
            registration: None,
        }));
        let registration = parts.session().register("tab", &tab).expect("a free key");
        tab.borrow_mut().registration = Some(registration);
        let parts = Rc::new(RefCell::new(parts));
        KIT.set(Some(Kit {
            parts: Rc::clone(&parts),
            gain: params.gain,
            drive,
            label,
            history,
            learn,
            tab,
        }));
        Unlisted { _parts: parts }
    }

    fn activate(&mut self, _: &ProcessSetup) -> Adder {
        unreachable!("never activated")
    }
}

/// A new instance, ticked until it has nothing left to tell.
fn settled<S: Shell>() -> S {
    let mut host = S::create();
    host.tick();
    assert_eq!(host.heard(), NOTHING, "a new instance tells nothing");
    host
}

/// The mapping of MIDI controller `number`, on any channel.
fn cc(number: u8) -> Mapping {
    Mapping::new(Source::any(Control::Cc(number)))
}

/// Controller `number` moves to `value`, as a block would bring it to
/// learn's audio side.
fn controller(number: u8, value: u8) {
    let mut events = EventList::with_capacity(1);
    let message = Midi1Message::ControlChange {
        controller: number,
        value,
    };
    let sent = Midi1Voice {
        group: 0,
        channel: 0,
        message,
    }
    .event(0);
    Upgrade::new().block(&[sent], 0, &mut events);
    kit(|kit| kit.learn.process(&events));
}

through_every_shell!(
    Unlisted:
    a_saved_value_that_changed_is_told_once_at_the_tick_that_captures_it,
    an_edit_to_a_parameter_made_at_run_time_is_told_and_one_to_a_listed_parameter_is_not,
    a_learned_mapping_is_told_and_what_its_controller_writes_is_not,
    changes_to_the_session_and_to_learned_mappings_in_one_tick_are_told_once,
    a_participant_of_the_plugin_s_own_is_told_at_the_tick_that_captures_it,
    a_load_is_never_told_and_drops_what_was_not_told_before_it,
);

fn changes_to_the_session_and_to_learned_mappings_in_one_tick_are_told_once<S: Shell>() {
    let mut host = settled::<S>();
    // A listed parameter's mapping is in the session's section, one made at
    // run time is saved outside it: both marks are set in one tick.
    kit(|kit| {
        let parts = kit.parts.borrow();
        let mut learn = parts.learn();
        learn.map(&kit.gain, cc(11)).expect("mapped");
        learn.map(&kit.drive, cc(7)).expect("mapped");
    });
    host.tick();
    assert_eq!(host.heard(), ONCE);
    host.tick();
    assert_eq!(host.heard(), NOTHING, "both marks were taken at the first");
    host.expect_faults(0);
}

fn a_participant_of_the_plugin_s_own_is_told_at_the_tick_that_captures_it<S: Shell>() {
    let mut host = settled::<S>();
    kit(|kit| kit.tab.borrow_mut().set(2));
    host.tick();
    assert_eq!(host.heard(), ONCE);
    let block: Json = serde_json::from_slice(&host.save()).expect("JSON");
    assert_eq!(block["extra"]["tab"], 2);
    host.tick();
    assert_eq!(host.heard(), NOTHING, "told once");

    // Reported again with the tab it holds: captured, and no change.
    kit(|kit| kit.tab.borrow_mut().set(2));
    host.tick();
    assert_eq!(host.heard(), NOTHING);
    host.expect_faults(0);
}

fn a_saved_value_that_changed_is_told_once_at_the_tick_that_captures_it<S: Shell>() {
    let mut host = settled::<S>();
    let label = |host: &S| -> Json {
        let block: Json = serde_json::from_slice(&host.save()).expect("JSON");
        block["extra"]["label"].clone()
    };

    kit(|kit| kit.label.set(String::from("Drums")));
    assert_eq!(host.heard(), NOTHING, "nothing before the tick");
    host.tick();
    assert_eq!(host.heard(), ONCE);
    assert_eq!(label(&host), "Drums");
    host.tick();
    assert_eq!(host.heard(), NOTHING, "told once");

    // Written again with the text it holds: captured, and no change.
    kit(|kit| kit.label.set(String::from("Drums")));
    host.tick();
    assert_eq!(host.heard(), NOTHING);

    // Captured at once by the plugin, and told at the next tick all the
    // same.
    kit(|kit| {
        kit.label.set(String::from("Bass"));
        kit.parts.borrow_mut().sync();
    });
    assert_eq!(label(&host), "Bass");
    assert_eq!(host.heard(), NOTHING);
    host.tick();
    assert_eq!(host.heard(), ONCE);

    // An undo's write is a change as the user's was.
    kit(|kit| assert!(kit.history.undo().expect("no gesture is open")));
    host.tick();
    assert_eq!(host.heard(), ONCE);
    assert_eq!(label(&host), "Drums");
    host.expect_faults(0);
}

fn an_edit_to_a_parameter_made_at_run_time_is_told_and_one_to_a_listed_parameter_is_not<
    S: Shell,
>() {
    let mut host = settled::<S>();

    // A user's write, then the plugin's own edit.
    kit(|kit| assert!(kit.parts.borrow().ui().write(&kit.drive, 0.5)));
    host.tick();
    assert_eq!(host.heard(), ONCE);
    host.tick();
    assert_eq!(host.heard(), NOTHING, "told once");
    kit(|kit| {
        let parts = kit.parts.borrow();
        let mut edit = parts.ui().edit();
        assert!(edit.write(&kit.drive, 0.25));
    });
    host.tick();
    assert_eq!(host.heard(), ONCE);

    // An undo of the user's write, and its redo.
    kit(|kit| assert!(kit.history.undo().expect("no gesture is open")));
    assert_eq!(kit(|kit| kit.drive.get()), 0.0);
    host.tick();
    assert_eq!(host.heard(), ONCE);
    kit(|kit| assert!(kit.history.redo().expect("no gesture is open")));
    assert_eq!(kit(|kit| kit.drive.get()), 0.5);
    host.tick();
    assert_eq!(host.heard(), ONCE);

    // Several in one tick are told once.
    kit(|kit| {
        let parts = kit.parts.borrow();
        assert!(parts.ui().write(&kit.drive, 0.75));
        assert!(parts.ui().write(&kit.drive, 1.0));
        kit.label.set(String::from("Drums"));
    });
    host.tick();
    assert_eq!(host.heard(), ONCE);

    // A listed parameter's edit is told as the edit it is, and a sync as
    // the re-read it asks for.
    kit(|kit| assert!(kit.parts.borrow().ui().write(&kit.gain, 5.0)));
    host.tick();
    assert_eq!(host.heard(), Heard { state: 0, other: 3 });
    kit(|kit| {
        let parts = kit.parts.borrow();
        assert!(parts.set().program().sync(kit.gain.id(), 2.0));
    });
    host.tick();
    assert_eq!(host.heard(), Heard { state: 0, other: 1 });
    host.expect_faults(0);
}

fn a_learned_mapping_is_told_and_what_its_controller_writes_is_not<S: Shell>() {
    let mut host = settled::<S>();

    // A listed parameter's mapping is in learn's section of the session.
    kit(|kit| {
        let parts = kit.parts.borrow();
        parts.learn().map(&kit.gain, cc(11)).expect("mapped");
    });
    host.tick();
    assert_eq!(host.heard(), ONCE);
    // One of a parameter made at run time is saved beside its value.
    kit(|kit| {
        let parts = kit.parts.borrow();
        parts.learn().map(&kit.drive, cc(7)).expect("mapped");
    });
    host.tick();
    assert_eq!(host.heard(), ONCE);
    let block: Json = serde_json::from_slice(&host.save()).expect("JSON");
    assert_eq!(block["runTime"]["drive"]["learned"]["number"], 7);
    host.tick();
    assert_eq!(host.heard(), NOTHING, "told once");

    // Both controllers move: the values land, and the host hears nothing.
    controller(11, 127);
    controller(7, 127);
    assert_eq!(kit(|kit| (kit.gain.get(), kit.drive.get())), (10.0, 1.0));
    host.tick();
    host.tick();
    assert_eq!(host.heard(), NOTHING);

    // A mapping removed is a change too.
    kit(|kit| {
        let parts = kit.parts.borrow();
        assert!(parts.learn().unmap(&kit.drive).is_some());
    });
    host.tick();
    assert_eq!(host.heard(), ONCE);
    host.expect_faults(0);
}

fn a_load_is_never_told_and_drops_what_was_not_told_before_it<S: Shell>() {
    // A project with every kind of unlisted state away from its default.
    let block = {
        let mut host = settled::<S>();
        kit(|kit| {
            let parts = kit.parts.borrow();
            kit.label.set(String::from("Drums"));
            assert!(parts.ui().write(&kit.drive, 0.5));
            assert!(parts.ui().write(&kit.gain, 5.0));
            parts.learn().map(&kit.gain, cc(11)).expect("mapped");
            parts.learn().map(&kit.drive, cc(7)).expect("mapped");
        });
        host.tick();
        host.tick();
        host.save()
    };
    let saved: Json = serde_json::from_slice(&block).expect("JSON");
    assert_eq!(saved["extra"]["label"], "Drums");
    assert_eq!(saved["runTime"]["drive"]["value"], 0.5);
    assert!(saved["runTime"]["drive"]["learned"].is_object(), "{saved}");
    let loaded = |host: &S| {
        assert_eq!(kit(|kit| kit.label.get()), "Drums");
        assert_eq!(kit(|kit| (kit.gain.get(), kit.drive.get())), (5.0, 0.5));
        let mapped = kit(|kit| {
            let parts = kit.parts.borrow();
            let learn = parts.learn();
            (
                learn.mapping(&kit.gain).copied(),
                learn.mapping(&kit.drive).copied(),
            )
        });
        assert_eq!(mapped, (Some(cc(11)), Some(cc(7))));
        assert_eq!(host.save(), block, "and saves back what it loaded");
    };

    // A new instance loads it, as a project that opens does.
    let mut host = S::create();
    assert_eq!(host.load(&block), Ok(()));
    for tick in 0..4 {
        host.tick();
        assert_eq!(host.heard(), NOTHING, "tick {tick}");
    }
    loaded(&host);

    // A second parameter made at run time, its mapping captured by a tick.
    let depth = kit(|kit| {
        let parts = kit.parts.borrow();
        let depth = parts.add("extra", FloatDecl::new("depth", "Depth", 0.0..=1.0, 0.0));
        parts.learn().map(&depth, cc(9)).expect("mapped");
        depth
    });
    host.tick();
    assert_eq!(host.heard(), ONCE);

    // It loads again over changes of every kind that no tick has told, the
    // second parameter let go among them, as a tree the load replaces is.
    kit(|kit| {
        kit.label.set(String::from("Bass"));
        kit.parts.borrow_mut().sync();
        let parts = kit.parts.borrow();
        assert!(parts.ui().write(&kit.drive, 0.75));
        assert!(parts.learn().unmap(&kit.gain).is_some());
        assert!(parts.learn().unmap(&kit.drive).is_some());
    });
    drop(depth);
    assert_eq!(host.load(&block), Ok(()));
    for tick in 0..4 {
        host.tick();
        assert_eq!(host.heard(), NOTHING, "tick {tick}");
    }
    loaded(&host);

    // A change after the load is told as any other.
    kit(|kit| kit.label.set(String::from("Keys")));
    host.tick();
    assert_eq!(host.heard(), ONCE);
    host.expect_faults(0);
}
