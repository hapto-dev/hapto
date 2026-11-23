//! What every shell does with a load its host made before an activation:
//! the activation applies it, so the processor it builds plays the loaded
//! session from its first block, with no tick between.

use std::cell::RefCell;
use std::rc::Rc;

use hapto_params::{Float, FloatDecl, Value};
use hapto_session::{ApplyError, Participant, Registration, Section};

use super::{Gain, Heard, Probe, Shell};
use crate::layout::Stereo;
use crate::{Block, Parts, Plugin, PluginInfo, ProcessSetup, Processor, Status};

/// What a block's samples hold with every part of the session loaded: the
/// listed gain's 1, the participant's 10, the saved value's 100 and the
/// 1000 of the parameter made at run time. Each part has a digit of its
/// own, so a sample says which of them arrived.
const LOADED: f32 = 1111.0;

/// What [`Sessioned`] hands the test, which runs it on the test's own
/// thread and reaches in as an editor's code would.
struct Kit {
    parts: Rc<RefCell<Parts>>,
    /// A participant of the plugin's own, saved under `offset`.
    offset: Rc<RefCell<Offset>>,
    /// A value saved under `bias`.
    bias: Value<f32>,
    /// A parameter made at run time, saved under `drive`.
    drive: Float,
}

/// A participant a plugin registers itself: a number its processor is built
/// from.
struct Offset {
    amount: f64,
    /// How many times a restore was applied to it.
    applies: u32,
    registration: Option<Registration>,
}

impl Offset {
    /// Sets the number and reports it, so the session captures it at the
    /// next tick.
    fn set(&mut self, amount: f64) {
        self.amount = amount;
        if let Some(registration) = &self.registration {
            registration.changed();
        }
    }
}

impl Participant for Offset {
    fn capture(&self) -> Section {
        Section::from(self.amount)
    }

    fn apply(&mut self, section: Option<&Section>) -> Result<(), ApplyError> {
        self.amount = section.and_then(Section::as_f64).unwrap_or(0.0);
        self.applies += 1;
        Ok(())
    }
}

thread_local! {
    static KIT: RefCell<Option<Kit>> = const { RefCell::new(None) };
}

fn kit<R>(f: impl FnOnce(&mut Kit) -> R) -> R {
    KIT.with(|kit| f(kit.borrow_mut().as_mut().expect("a plugin was made")))
}

/// Keeps its parts, and builds its processor from each kind of state a load
/// only stores: a participant's section and a saved value, both read as it
/// activates, and a parameter made at run time, which each block reads.
struct Sessioned {
    _parts: Rc<RefCell<Parts>>,
    gain: Gain,
    offset: Rc<RefCell<Offset>>,
    bias: Value<f32>,
    drive: Float,
}

/// Adds the listed gain and the three parts of the session to every sample.
struct Sum {
    gain: Gain,
    /// The participant's number and the saved value, as the activation
    /// found them.
    built_from: f32,
    drive: Float,
}

impl Processor for Sum {
    type Layout = Stereo;

    fn process(&mut self, mut block: Block<'_, Stereo>) -> Status {
        let add = self.gain.gain.get() + self.built_from + self.drive.get();
        for channel in block.audio.channels_mut() {
            for sample in channel {
                *sample += add;
            }
        }
        Status::Continue
    }
}

impl Plugin for Sessioned {
    const INFO: PluginInfo = Probe::INFO;
    type Params = Gain;
    type Processor = Sum;

    fn new(gain: Gain, mut parts: Parts) -> Self {
        let bias = parts.saved_value("bias", 0.0_f32).expect("a free key");
        let drive = parts.add("", FloatDecl::new("drive", "Drive", 0.0..=1000.0, 0.0));
        let offset = Rc::new(RefCell::new(Offset {
            amount: 0.0,
            applies: 0,
            registration: None,
        }));
        let registration = parts
            .session()
            .register("offset", &offset)
            .expect("a free key");
        offset.borrow_mut().registration = Some(registration);
        let parts = Rc::new(RefCell::new(parts));
        KIT.set(Some(Kit {
            parts: Rc::clone(&parts),
            offset: Rc::clone(&offset),
            bias: bias.clone(),
            drive: drive.clone(),
        }));
        Sessioned {
            _parts: parts,
            gain,
            offset,
            bias,
            drive,
        }
    }

    fn activate(&mut self, _: &ProcessSetup) -> Sum {
        Sum {
            gain: self.gain.clone(),
            built_from: self.offset.borrow().amount as f32 + self.bias.get(),
            drive: self.drive.clone(),
        }
    }
}

/// The block a host saved with every part of the session away from its
/// default, as [`LOADED`] lists them.
fn saved<S: Shell>() -> Vec<u8> {
    let mut host = S::create();
    host.write(0.1);
    kit(|kit| {
        kit.offset.borrow_mut().set(10.0);
        kit.bias.set(100.0);
        assert!(kit.parts.borrow().ui().write(&kit.drive, 1000.0));
    });
    host.tick();
    host.save()
}

/// How many restores the participant has had applied.
fn applies() -> u32 {
    kit(|kit| kit.offset.borrow().applies)
}

through_every_shell!(
    Sessioned:
    a_load_before_an_activation_plays_in_the_first_block,
    an_activation_while_active_applies_a_load_made_since_the_last,
    a_load_an_activation_applied_is_applied_once_and_never_told,
    a_participant_busy_at_an_activation_takes_the_load_at_the_next_tick,
);

fn a_load_before_an_activation_plays_in_the_first_block<S: Shell>() {
    let block = saved::<S>();
    // A project that opens and plays at once: no tick between.
    let mut host = S::create();
    assert_eq!(host.load(&block), Ok(()));
    assert_eq!(host.activate(), Ok(()));
    assert_eq!(host.play(0.0), [LOADED; 8]);
    host.expect_faults(0);
}

fn an_activation_while_active_applies_a_load_made_since_the_last<S: Shell>() {
    let block = saved::<S>();
    let mut host = S::create();
    assert_eq!(host.activate(), Ok(()));
    assert_eq!(host.play(0.0), [0.0; 8], "a new instance's defaults");
    assert_eq!(host.load(&block), Ok(()));
    assert_eq!(host.activate(), Ok(()), "again, while active");
    assert_eq!(host.play(0.0), [LOADED; 8]);
    host.expect_faults(0);
}

fn a_load_an_activation_applied_is_applied_once_and_never_told<S: Shell>() {
    let block = saved::<S>();
    let mut host = S::create();
    host.tick();
    assert_eq!(host.heard(), Heard::default(), "a new instance");
    let before = applies();
    assert_eq!(host.load(&block), Ok(()));
    assert_eq!(host.activate(), Ok(()));
    assert_eq!(applies(), before + 1, "applied by the activation");
    for tick in 0..2 {
        host.tick();
        assert_eq!(host.heard(), Heard::default(), "tick {tick}");
    }
    assert_eq!(applies(), before + 1, "and by no tick after it");
    assert_eq!(host.save(), block, "and saves back what it loaded");
    host.expect_faults(0);
}

fn a_participant_busy_at_an_activation_takes_the_load_at_the_next_tick<S: Shell>() {
    let block = saved::<S>();
    let mut host = S::create();
    assert_eq!(host.load(&block), Ok(()));
    // Held as an editor's code holds it across a call that reaches the
    // host, which activates from inside it.
    let offset = kit(|kit| Rc::clone(&kit.offset));
    let held = offset.borrow();
    assert_eq!(host.activate(), Ok(()), "not refused");
    drop(held);
    // The pass is not finished while a participant is owed its section, so
    // the parameter made at run time waits with it.
    assert_eq!(
        host.play(0.0),
        [101.0; 8],
        "the listed gain and the saved value, which were free"
    );
    host.tick();
    assert_eq!(kit(|kit| kit.offset.borrow().amount), 10.0, "applied");
    assert_eq!(
        host.play(0.0),
        [1101.0; 8],
        "the parameter is read by each block, the participant as it activates"
    );
    assert_eq!(host.activate(), Ok(()));
    assert_eq!(host.play(0.0), [LOADED; 8]);
    host.expect_faults(0);
}
