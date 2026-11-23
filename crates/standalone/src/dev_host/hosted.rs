//! The plugin as the dev host holds it, in one format: through the VST3
//! exporter's COM interfaces or through the C interface, each by Hapto's own
//! test host, in process.
//!
//! Each test host is made once for the run and kept for its whole life: its
//! audio side borrows it, and the device's callback owns that side. A switch
//! of format starts the app again instead. A change of the audio makes a new
//! side from the same host, once the device has dropped the old one.

#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use hapto_host::layout::Width;
use hapto_host::vst3::{HostCall, RESTART_LATENCY, RESTART_VALUES, TestView};
use hapto_host::{Event, Plugin, cabi, vst3};

use super::format::{Call, Format};
use super::pending::Pending;
use crate::block::Render;

/// The most changes one block carries: the VST3 test host's audio side sends
/// up to this many queues with no allocation.
const MAX_CHANGES: usize = 64;

/// A parameter as the host lists it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Param {
    /// The host's id for it in its calls: VST3's id, or the C interface's
    /// index.
    pub(crate) id: u32,
    /// The id Hapto's exporters give hosts, which a key hashes to.
    pub(crate) host_id: u32,
    /// What the host names it by, as the settings window shows it: the key in
    /// the C interface, the id in VST3.
    pub(crate) handle: String,
    /// Its name.
    pub(crate) title: String,
    /// Its unit.
    pub(crate) units: String,
    /// Its values but one, or 0 for a continuous one.
    pub(crate) steps: u32,
    /// What kind of value it holds, in words: `continuous, dB`, `choice of
    /// 3`, `the host's bypass`.
    pub(crate) kind: String,
}

/// The kind of a parameter in words: the bypass, a value that is off or on,
/// one of `values` choices, or a continuous one, with its unit.
fn kind_text(bypass: bool, switch: bool, values: Option<u32>, units: &str) -> String {
    let kind = if bypass {
        "the host's bypass".to_owned()
    } else if switch {
        "off or on".to_owned()
    } else if let Some(values) = values {
        format!("choice of {values}")
    } else {
        "continuous".to_owned()
    };
    if units.is_empty() || bypass || switch {
        kind
    } else {
        format!("{kind}, {units}")
    }
}

/// The plugin, in the format the dev host runs it through. Thread: main.
pub(crate) trait Hosted {
    /// Returns its format.
    fn format(&self) -> Format;
    /// Returns its parameters, as the host lists them.
    fn params(&self) -> &[Param];
    /// Returns the parameter at `index` as 0 to 1, and its text.
    fn value(&self, index: usize) -> (f64, String);
    /// Sets the parameter at `index`, as a host's own control does.
    fn set(&self, index: usize, normalized: f64);
    /// Runs a main-thread tick, where the format leaves it to the host.
    fn tick(&self);
    /// Returns what the plugin told the host since the last call.
    fn calls(&self) -> Vec<Call>;
    /// Returns how many faults the instance has had.
    fn faults(&self) -> u64;
    /// Returns the latency the plugin answers.
    fn latency(&self) -> u32;
    /// Returns the saved block, as the host saves it with a project.
    fn save(&self) -> Option<Vec<u8>>;
    /// Loads a saved block.
    ///
    /// # Errors
    ///
    /// Returns the format's answer when the plugin refused it, which in
    /// either format says no more than that it was refused.
    fn load(&self, block: &[u8]) -> Result<()>;
    /// Returns the editor's view, where the format has one.
    fn view(&self) -> Option<TestView>;
    /// Stops processing and deactivates, as a host does before it changes the
    /// sample rate or the most frames a block holds. No block may run
    /// meanwhile.
    fn stop(&self);
    /// Activates for `sample_rate`, blocks of up to `max_frames` and
    /// `layout`, starts processing, and returns the audio side.
    ///
    /// # Errors
    ///
    /// Returns which call the plugin refused.
    fn start(&self, sample_rate: f64, max_frames: usize, layout: Width) -> Result<Box<dyn Render>>;
}

/// Makes the plugin in `format`.
pub(crate) fn host<P: Plugin>(format: Format) -> Box<dyn Hosted> {
    match format {
        Format::Vst3 => Box::new(Vst3::<P>::new()),
        Format::Cabi => Box::new(Cabi::<P>::new()),
    }
}

/// The plugin through the VST3 exporter, ticked by the exporter's own timer
/// on the window's message loop, as in a host.
struct Vst3<P: Plugin> {
    host: &'static vst3::TestHost<P>,
    params: Vec<Param>,
    pending: Arc<Pending>,
    /// Whether it processes, so a block carries a control's change.
    processing: Cell<bool>,
}

impl<P: Plugin> Vst3<P> {
    fn new() -> Self {
        let host: &'static _ = Box::leak(Box::new(vst3::TestHost::<P>::with_timer()));
        // A host shows no hidden parameter: the proxies a plugin's MIDI
        // controllers reach it through.
        let params: Vec<Param> = (0..host.parameter_count())
            .filter_map(|index| host.parameter(index))
            .filter(|info| !info.is_hidden)
            .map(|info| {
                let steps = u32::try_from(info.step_count).unwrap_or(0);
                let values = (steps > 0).then_some(steps + 1);
                Param {
                    id: info.id,
                    host_id: info.id,
                    handle: format!("id {:#010x}", info.id),
                    kind: kind_text(info.is_bypass, steps == 1, values, &info.units),
                    title: info.title,
                    units: info.units,
                    steps,
                }
            })
            .collect();
        let pending = Arc::new(Pending::new(params.iter().map(|p| p.id).collect()));
        Vst3 {
            host,
            params,
            pending,
            processing: Cell::new(false),
        }
    }

    fn title(&self, id: u32) -> Option<&str> {
        self.params
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.title.as_str())
    }
}

impl<P: Plugin> Hosted for Vst3<P> {
    fn format(&self) -> Format {
        Format::Vst3
    }

    fn params(&self) -> &[Param] {
        &self.params
    }

    fn value(&self, index: usize) -> (f64, String) {
        let Some(param) = self.params.get(index) else {
            return (0.0, String::new());
        };
        let normalized = self.host.normalized(param.id);
        let text = self.host.text(param.id, normalized).unwrap_or_default();
        (normalized, text)
    }

    fn set(&self, index: usize, normalized: f64) {
        if self.processing.get() {
            // With the block, as a host sends its own controls' changes while
            // it processes; a controller write then is ignored.
            self.pending.set(index, normalized);
        } else if let Some(param) = self.params.get(index) {
            // No block runs to carry it, so the controller takes it, as a
            // host writes it then.
            let _written = self
                .host
                .set_normalized(param.id, normalized.clamp(0.0, 1.0));
        }
    }

    fn tick(&self) {}

    fn calls(&self) -> Vec<Call> {
        let known = [
            (u64::from(RESTART_VALUES.cast_unsigned()), "values"),
            (u64::from(RESTART_LATENCY.cast_unsigned()), "latency"),
        ];
        self.host
            .calls()
            .into_iter()
            .map(|call| match call {
                HostCall::Begin(id) => Call::edit("beginEdit", self.title(id), None),
                HostCall::Perform(id, value) => {
                    Call::edit("performEdit", self.title(id), Some(value))
                }
                HostCall::End(id) => Call::edit("endEdit", self.title(id), None),
                HostCall::Restart(flags) => {
                    let flags = u64::from(flags.cast_unsigned());
                    Call::restart("restartComponent", flags, &known)
                }
                HostCall::Dirty(state) => Call::dirty("setDirty", state),
            })
            .collect()
    }

    fn faults(&self) -> u64 {
        self.host.faults()
    }

    fn latency(&self) -> u32 {
        self.host.latency()
    }

    fn save(&self) -> Option<Vec<u8>> {
        self.host.save().ok()
    }

    fn load(&self, block: &[u8]) -> Result<()> {
        self.host.load(block).context("VST3 setState")
    }

    fn view(&self) -> Option<TestView> {
        self.host.view()
    }

    fn stop(&self) {
        self.processing.set(false);
        // Refused when not processing or not active, as after a start that
        // failed part of the way: nothing to undo then.
        let _stopped = self.host.stop();
        let _deactivated = self.host.deactivate();
    }

    fn start(&self, sample_rate: f64, max_frames: usize, layout: Width) -> Result<Box<dyn Render>> {
        self.host
            .arrange(layout)
            .context("VST3 setBusArrangements")?;
        self.host
            .setup(sample_rate, max_frames)
            .context("VST3 setupProcessing")?;
        self.host.activate().context("VST3 setActive")?;
        self.host.start().context("VST3 setProcessing")?;
        self.processing.set(true);
        Ok(Box::new(Vst3Audio {
            side: self.host.audio_side(),
            pending: Arc::clone(&self.pending),
            changes: [(0, 0.0); MAX_CHANGES],
        }))
    }
}

/// The VST3 host's audio side, with the settings window's changes for each
/// block.
struct Vst3Audio {
    side: vst3::AudioSide<'static>,
    pending: Arc<Pending>,
    changes: [(u32, f64); MAX_CHANGES],
}

impl Render for Vst3Audio {
    // Open: the dev host hands the formats no MIDI yet.
    fn render(&mut self, inputs: &[&[f32]], outputs: &mut [&mut [f32]], _midi: &[Event]) {
        let frames = block_length(inputs, outputs);
        let count = self.pending.take(&mut self.changes);
        let changes = self.changes.get(..count).unwrap_or_default();
        // A refusal is the exporter's to count as a fault, which the
        // settings window shows.
        let _played = self.side.process(inputs, outputs, frames, changes);
    }
}

/// The plugin through the C interface, ticked by the dev host, as the JUCE
/// adapter ticks it.
struct Cabi<P: Plugin> {
    host: &'static cabi::TestHost<P>,
    params: Vec<Param>,
}

impl<P: Plugin> Cabi<P> {
    fn new() -> Self {
        let host: &'static _ = Box::leak(Box::new(cabi::TestHost::<P>::new()));
        let count = u32::try_from(host.parameter_count()).unwrap_or(u32::MAX);
        let params = (0..count)
            .filter_map(|index| host.parameter(index).map(|info| (index, info)))
            .map(|(index, info)| {
                let bypass = info.flags & cabi::PARAM_BYPASS != 0;
                let choice = (info.kind == cabi::KIND_CHOICE).then_some(info.steps);
                let switch = info.kind == cabi::KIND_BOOL;
                Param {
                    id: index,
                    host_id: info.host_id,
                    kind: kind_text(bypass, switch, choice, &info.unit),
                    handle: info.key,
                    title: info.name,
                    units: info.unit,
                    steps: info.steps.saturating_sub(1),
                }
            })
            .collect();
        Cabi { host, params }
    }

    fn title(&self, index: u32) -> Option<&str> {
        self.params
            .get(usize::try_from(index).ok()?)
            .map(|p| p.title.as_str())
    }
}

impl<P: Plugin> Hosted for Cabi<P> {
    fn format(&self) -> Format {
        Format::Cabi
    }

    fn params(&self) -> &[Param] {
        &self.params
    }

    fn value(&self, index: usize) -> (f64, String) {
        let Some(param) = self.params.get(index) else {
            return (0.0, String::new());
        };
        let normalized = self.host.get(param.id);
        (normalized, self.host.text(param.id, normalized, 64).0)
    }

    fn set(&self, index: usize, normalized: f64) {
        if let Some(param) = self.params.get(index) {
            self.host.set(param.id, normalized.clamp(0.0, 1.0));
        }
    }

    fn tick(&self) {
        self.host.idle();
    }

    fn calls(&self) -> Vec<Call> {
        let known = [
            (u64::from(cabi::RESTART_VALUES), "values"),
            (u64::from(cabi::RESTART_LATENCY), "latency"),
            (u64::from(cabi::RESTART_STATE), "state"),
        ];
        self.host
            .calls()
            .into_iter()
            .map(|call| match call {
                cabi::HostCall::Begin(index) => Call::edit("begin_edit", self.title(index), None),
                cabi::HostCall::Perform(index, value) => {
                    Call::edit("perform_edit", self.title(index), Some(value))
                }
                cabi::HostCall::End(index) => Call::edit("end_edit", self.title(index), None),
                cabi::HostCall::Restart(flags) => {
                    Call::restart("restart", u64::from(flags), &known)
                }
            })
            .collect()
    }

    fn faults(&self) -> u64 {
        self.host.faults()
    }

    fn latency(&self) -> u32 {
        self.host.latency()
    }

    fn save(&self) -> Option<Vec<u8>> {
        self.host.save().ok()
    }

    fn load(&self, block: &[u8]) -> Result<()> {
        self.host.load(block).context("the C interface's load")
    }

    fn view(&self) -> Option<TestView> {
        None
    }

    fn stop(&self) {
        self.host.deactivate();
    }

    fn start(&self, sample_rate: f64, max_frames: usize, layout: Width) -> Result<Box<dyn Render>> {
        let frames = u32::try_from(max_frames).unwrap_or(u32::MAX);
        self.host
            .activate(sample_rate, frames, layout)
            .context("the C interface's activate")?;
        Ok(Box::new(CabiAudio {
            side: self.host.audio_side(),
        }))
    }
}

/// The C interface host's audio side.
struct CabiAudio {
    side: cabi::AudioSide<'static>,
}

impl Render for CabiAudio {
    fn render(&mut self, inputs: &[&[f32]], outputs: &mut [&mut [f32]], _midi: &[Event]) {
        let frames = block_length(inputs, outputs);
        let _status = self.side.process(inputs, outputs, frames);
    }
}

/// The frames a block holds: the [`Player`](crate::block::Player) gives
/// every channel the same length.
fn block_length(inputs: &[&[f32]], outputs: &[&mut [f32]]) -> usize {
    outputs
        .first()
        .map(|c| c.len())
        .or_else(|| inputs.first().map(|c| c.len()))
        .unwrap_or(0)
}
