//! The example gain: a gain in dB, a bypass, a mode with three names, a label
//! that is saved and undoable, and a gain that a MIDI controller can be
//! learned to.
//!
//! It shows a plugin that uses the parts Hapto builds for it: parameters,
//! saved state, undo, the session, learn and the VST3 export. The guide
//! `docs/first-plugin.md` walks through it.
//!
//! The page's Learn button arms learn on the gain, and the next controller a
//! host or the app's MIDI inputs send drives it from then on.

#[cfg(feature = "vst3")]
hapto_host::export_vst3!(Gain);

mod page;

pub use page::PAGE;

use std::cell::Cell;
use std::rc::Rc;

use hapto_host::layout::Stereo;
use hapto_host::{Block, Editor, Parts, Plugin, PluginInfo, ProcessSetup, Processor, Status};
use hapto_learn::{Config, LearnError, Takes};
use hapto_params::{Bool, Changes, Choice, Choices, Float, Params, UiParams, Value};
use hapto_undo::{Bound, History};

const LABEL_KEY: &str = "label";
const DEFAULT_LABEL: &str = "Untitled";
/// Where the gain's MIDI learn is, as the page reads it.
const LEARN_OFF: &str = "off";
const LEARN_ARMED: &str = "armed";
const LEARN_LEARNED: &str = "learned";

/// What each mode does to the sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Choices)]
enum Mode {
    /// Gain only.
    Clean,
    /// Gain, then a soft clip.
    Warm,
    /// Gain with three times the drive, then a soft clip.
    Hot,
}

/// The parameters, in the order hosts list them, each declared where it is
/// held.
#[derive(Debug, Clone, Params)]
pub struct GainParams {
    #[param(
        "gain", "Gain", -60.0..=12.0, 0.0,
        step = 0.1, unit = "dB",
    )]
    gain: Float,
    #[param("mode", "Mode", Mode::Clean)]
    mode: Choice<Mode>,
    #[param("bypass", "Bypass", false, bypass)]
    bypass: Bool,
}

/// The plugin, on the main thread.
#[derive(Debug)]
pub struct Gain {
    parts: Parts,
    params: GainParams,
    history: History,
    label: Value<String>,
    /// Set by the page's Learn button; the next idle arms learn on the gain.
    learn_asked: Rc<Cell<bool>>,
    /// Where the gain's MIDI learn is, for the page to show: `off`, `armed`
    /// or `learned`.
    learn_state: Value<String>,
}

impl Gain {
    /// The label now.
    pub fn label(&self) -> String {
        self.label.get()
    }

    /// Renames the instance: one undo step, saved with the plugin from the
    /// next tick.
    pub fn set_label(&self, label: &str) {
        self.label.set(label.to_owned());
    }

    /// The parameter handle for an editor's writes, which undo records.
    pub fn ui(&self) -> &UiParams {
        self.parts.ui()
    }

    /// The undo history of the parameters and the label.
    pub fn history(&self) -> &History {
        &self.history
    }

    /// Arms the gain for MIDI learn: the next controller a block brings
    /// drives it, once the next tick commits it.
    ///
    /// # Errors
    ///
    /// As [`Learn::arm`](hapto_learn::Learn::arm).
    pub fn learn_gain(&mut self) -> Result<(), LearnError> {
        let gain = self.params.gain.id();
        self.parts.learn().arm(gain, Takes::CONTROLLER)
    }

    /// Whether a controller is learned to the gain.
    pub fn gain_is_learned(&self) -> bool {
        self.parts.learn().mapping(self.params.gain.id()).is_some()
    }

    /// Asks for learn on the gain at the next idle, as the page's Learn
    /// button does.
    pub fn ask_to_learn(&self) {
        self.learn_asked.set(true);
    }

    /// Where the gain's MIDI learn is, as the page shows it: `off`, `armed`
    /// or `learned`.
    pub fn learn_state(&self) -> String {
        self.learn_state.get()
    }
}

impl Plugin for Gain {
    const INFO: PluginInfo = hapto_host::manifest_info! {
        midi_input: true,
        midi_output: false,
    };

    type Params = GainParams;

    type Processor = GainProcessor;

    fn new(params: GainParams, mut parts: Parts) -> Self {
        let history = parts.undo(Bound::default());
        parts.midi_learn(Config::default());

        let label = parts
            .saved_value(LABEL_KEY, DEFAULT_LABEL.to_owned())
            .expect("a free key");
        history.enroll(&label, "Rename").expect("a new value");

        let learn_state = Value::new(LEARN_OFF.to_owned(), parts.set());
        Gain {
            parts,
            params,
            history,
            label,
            learn_asked: Rc::new(Cell::new(false)),
            learn_state,
        }
    }

    fn activate(&mut self, _setup: &ProcessSetup) -> GainProcessor {
        GainProcessor::new(self.params.clone())
    }

    fn editor(&mut self) -> Option<Box<dyn Editor>> {
        page::editor(self)
    }

    /// Arms learn when the page asked, and tells the page where learn is.
    fn idle(&mut self) {
        if self.learn_asked.take() {
            // Learn is on from `new`, and the gain is a parameter, so arming
            // it is never refused.
            let _ = self.learn_gain();
        }
        let state = if self.parts.learn().armed().is_some() {
            LEARN_ARMED
        } else if self.gain_is_learned() {
            LEARN_LEARNED
        } else {
            LEARN_OFF
        };
        if self.learn_state.get() != state {
            self.learn_state.set(state.to_owned());
        }
    }
}

/// The audio-thread half: a gain and a mode over the first two channels.
#[derive(Debug)]
pub struct GainProcessor {
    params: GainParams,
    /// What the fields below were last read from.
    changes: Changes,
    gain: f32,
    mode: Mode,
    bypassed: bool,
}

impl GainProcessor {
    fn new(params: GainParams) -> Self {
        let mut processor = GainProcessor {
            params,
            changes: Changes::new(),
            gain: 1.0,
            mode: Mode::Clean,
            bypassed: false,
        };
        processor.retune();
        processor
    }

    /// Reads the parameters again when one moved. Atomics only.
    fn retune(&mut self) {
        let GainParams { gain, mode, bypass } = &self.params;
        if !self.changes.moved([gain, mode, bypass]) {
            return;
        }
        self.gain = 10.0_f32.powf(gain.get() / 20.0);
        self.mode = mode.get();
        self.bypassed = bypass.get();
    }
}

impl Processor for GainProcessor {
    type Layout = Stereo;

    fn process(&mut self, mut block: Block<'_, Stereo>) -> Status {
        self.retune();
        if self.bypassed {
            return Status::Continue;
        }
        let (gain, mode) = (self.gain, self.mode);
        for channel in block.audio.channels_mut() {
            for sample in channel {
                *sample = match mode {
                    Mode::Clean => *sample * gain,
                    Mode::Warm => (*sample * gain).tanh(),
                    Mode::Hot => (*sample * gain * 3.0).tanh(),
                };
            }
        }
        Status::Continue
    }
}
