//! The standalone exporter: a plugin as an app of its own, with no host.
//!
//! An app makes a [`Standalone`] on its main thread, which becomes the
//! plugin's, activates it for its audio device, hands the [`StandaloneAudio`]
//! side to the device's callback, ticks it about 60 times a second, and opens
//! the plugin's editor ([`StandaloneEditor`]) in a window of its own. It is
//! the same engine every exporter drives, so activation, the block, the tick,
//! the saved block and teardown keep the rules they keep in a host.
//!
//! The app is the host. An edit the plugin or its page makes lands in the
//! parameter store, and there is no one else to tell; the saved block is the
//! app's to keep between runs. No automation and no MIDI reach the plugin.
//!
//! `hapto-standalone` is such an app, for any plugin.

#[cfg(test)]
mod tests;

use std::error::Error;
use std::fmt;
use std::mem::ManuallyDrop;
use std::rc::Rc;
use std::sync::Arc;

use hapto_params::{HostEvent, ParamSet};

use crate::engine::{TAIL_INFINITE, TAIL_NONE};
use raw_window_handle::{HandleError, HasWindowHandle};

use crate::boundary::{ParentWindow, guard};
use crate::engine::{EditorSlot, Engine, HostLink, Opened, Outcome, Release, Restart, Setup};
use crate::layout::Width;
use crate::{Event, LoadError, Plugin, Size, Status, Tail, widths};

/// The most channels a block of the audio side carries each way: its lists
/// of them live on the audio thread's stack, so a block allocates nothing.
pub const MAX_CHANNELS: usize = 32;

/// The standalone app's link: it is the host, so nothing is told.
#[derive(Debug, Clone, Copy)]
struct Link;

impl HostLink for Link {
    const ACTIVATION_TELLS_LATENCY: bool = true;
    const QUEUES: bool = false;

    fn tell(&self, _set: &ParamSet, _event: HostEvent) {}

    fn restart(&self, _restart: Restart) {}

    fn state_changed(&self) {}
}

/// One instance of a plugin, as a standalone app drives it.
///
/// Thread: main. It is made on the app's main thread, which becomes the
/// plugin's, and is not `Send`. Only its [`audio`](Self::audio) side goes to
/// the audio thread.
///
/// The app drops the audio side, by stopping its device, before this. One
/// still alive then could reach the instance, so the instance leaks, as an
/// exporter's does when its host might still call it, and the leak is a
/// fault. An editor may outlive it: the last editor to drop then frees the
/// plugin, on this thread.
///
/// # Examples
///
/// ```
#[doc = include_str!("../doc/gain.rs")]
/// use hapto_host::standalone::Standalone;
///
/// let app = Standalone::<Gain>::new();
/// app.activate(48_000.0, 4, Width::STEREO)?;
///
/// // The device's callback owns the audio side.
/// let mut audio = app.audio();
/// let input = [1.0_f32; 4];
/// let (mut left, mut right) = ([0.0_f32; 4], [0.0_f32; 4]);
/// let inputs = [&input[..], &input[..]];
/// // Four frames, no MIDI in, and none of what it sends kept.
/// let outputs = &mut [&mut left[..], &mut right[..]];
/// audio.process(&inputs, outputs, 4, &[], |_| {});
/// assert_eq!(left, [1.0; 4], "0 dB");
///
/// // The app's tick, about 60 times a second; its saved block, between runs.
/// app.tick();
/// let saved = app.save();
/// app.load(&saved)?;
/// drop(audio);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct Standalone<P: Plugin> {
    /// Dropped by hand, inside a guard: a plugin whose drop panics must not
    /// unwind into the app.
    engine: ManuallyDrop<Arc<Engine<P, Link>>>,
    /// Cloned into each editor it hands out, so its count is the editors
    /// alive and one. An `Rc`, it also keeps the instance on the thread that
    /// made it.
    editors: Rc<()>,
}

impl<P: Plugin> fmt::Debug for Standalone<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Standalone")
            .field("plugin", &P::INFO.name)
            .finish_non_exhaustive()
    }
}

impl<P: Plugin> Default for Standalone<P> {
    fn default() -> Self {
        Self::new()
    }
}

/// Why an activation was refused.
///
/// # Examples
///
/// ```
#[doc = include_str!("../doc/gain.rs")]
/// use hapto_host::standalone::{Refused, Standalone};
///
/// let app = Standalone::<Gain>::new();
/// // The gain declares stereo only.
/// let refused = app.activate(48_000.0, 512, Width::MONO);
/// assert_eq!(refused, Err(Refused::Layout(Width::MONO)));
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum Refused {
    /// The plugin does not declare this layout.
    Layout(Width),
    /// A sample rate that is not a positive finite number.
    SampleRate(f64),
    /// The plugin is inside a call of its own, the app having called in from
    /// inside it; counted as a fault.
    Busy,
    /// The plugin panicked as it activated; counted as a fault. A panic after
    /// the new processor was in place, as the plugin took back the one before
    /// or was asked its latency, answers this too, though blocks then play
    /// through the new processor.
    Panicked,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refused::Layout(layout) => write!(
                f,
                "the plugin declares no layout of {} in and {} out",
                layout.inputs, layout.outputs
            ),
            Refused::SampleRate(rate) => write!(f, "{rate} is no sample rate"),
            Refused::Busy => f.write_str("the plugin is busy in a call of its own"),
            Refused::Panicked => f.write_str("the plugin panicked as it activated"),
        }
    }
}

impl Error for Refused {}

/// Why a saved block was not loaded.
///
/// # Examples
///
/// ```
#[doc = include_str!("../doc/gain.rs")]
/// use hapto_host::LoadError;
/// use hapto_host::standalone::{LoadRefused, Standalone};
///
/// let app = Standalone::<Gain>::new();
/// let refused = app.load(b"not a block");
/// assert!(matches!(refused, Err(LoadRefused::Block(LoadError::NotJson))));
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub enum LoadRefused {
    /// The block was refused for the reason it holds. It reads as that
    /// reason, and its source is the reason's.
    Block(LoadError),
    /// The plugin panicked inside the load, in its migration say; counted as
    /// a fault.
    Panicked,
}

impl fmt::Display for LoadRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadRefused::Block(error) => fmt::Display::fmt(error, f),
            LoadRefused::Panicked => {
                f.write_str("the plugin panicked as it loaded the saved block")
            }
        }
    }
}

impl Error for LoadRefused {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            LoadRefused::Block(error) => error.source(),
            LoadRefused::Panicked => None,
        }
    }
}

impl<P: Plugin> Standalone<P> {
    /// Creates the plugin on the calling thread, which becomes its main
    /// thread.
    ///
    /// # Panics
    ///
    /// When the plugin's declarations are refused, once a line with the
    /// reason is written, and when the plugin's own `new` panics: the app has
    /// no instance to run.
    pub fn new() -> Self {
        Standalone {
            engine: ManuallyDrop::new(Arc::from(Engine::new(Some(Link)))),
            editors: Rc::new(()),
        }
    }

    /// Activates the plugin for `sample_rate`, blocks of up to `max_frames`
    /// and `layout`, building a fresh processor, as a host's activation does.
    /// A longer block plays in pieces of `max_frames`.
    ///
    /// # Errors
    ///
    /// [`Refused`] says why it did not activate.
    pub fn activate(
        &self,
        sample_rate: f64,
        max_frames: usize,
        layout: Width,
    ) -> Result<(), Refused> {
        if !widths::<P>().contains(&layout) {
            return Err(Refused::Layout(layout));
        }
        let setup = Setup::new(sample_rate, max_frames).ok_or(Refused::SampleRate(sample_rate))?;
        let main = self.engine.main().ok_or(Refused::Busy)?;
        let activated = self
            .engine
            .guard(None, || Some(self.engine.activate(&main, setup, layout)));
        match activated {
            Some(true) => Ok(()),
            Some(false) => Err(Refused::Busy),
            None => Err(Refused::Panicked),
        }
    }

    /// Takes the processor back from the audio thread, once a running block
    /// finishes, and hands it to the plugin. Blocks are silent until the
    /// next activation.
    pub fn deactivate(&self) {
        if let Some(main) = self.engine.main() {
            self.engine.guard(false, || self.engine.deactivate(&main));
        }
    }

    /// Returns a side for the audio thread, which owns it.
    pub fn audio(&self) -> StandaloneAudio<P> {
        StandaloneAudio {
            engine: Arc::clone(&self.engine),
        }
    }

    /// Ticks the plugin: its parts, its idle, and its open editor, as an
    /// exporter's timer does. A panic inside is caught and counted.
    pub fn tick(&self) {
        self.engine.idle();
    }

    /// Asks the plugin for its editor, to open in a window of the app's.
    /// `None` when the plugin has none, or when it is busy in a call of its
    /// own, which counts as a fault.
    pub fn editor(&self) -> Option<StandaloneEditor<P>> {
        let main = self.engine.main()?;
        let slot = self.engine.guard(None, || self.engine.editor(&main))?;
        Some(StandaloneEditor {
            slot: ManuallyDrop::new(slot),
            engine: ManuallyDrop::new(Arc::clone(&self.engine)),
            _counted: Rc::clone(&self.editors),
        })
    }

    /// Writes the saved block, as a host saves it with a project.
    pub fn save(&self) -> Vec<u8> {
        self.engine.save()
    }

    /// Loads a saved block. A panic inside, in the plugin's migration say,
    /// is caught and counted as a fault, and refuses the block, as a host's
    /// load is refused.
    ///
    /// # Errors
    ///
    /// [`LoadRefused`] says why the block did not take. The first block
    /// refused also writes a line with the reason.
    pub fn load(&self, bytes: &[u8]) -> Result<(), LoadRefused> {
        self.engine.guard(Err(LoadRefused::Panicked), || {
            self.engine.load_saved(bytes).map_err(LoadRefused::Block)
        })
    }

    /// Returns the plugin's latency in frames, as the main thread last read
    /// it.
    pub fn latency(&self) -> u32 {
        self.engine.latency()
    }

    /// Returns the plugin's tail, as the main thread last read it.
    pub fn tail(&self) -> Tail {
        match self.engine.tail() {
            TAIL_NONE => Tail::None,
            TAIL_INFINITE => Tail::Infinite,
            frames => Tail::Frames(frames),
        }
    }

    /// Asks the next block to reset the processor first, as a host's stop
    /// does: for an app that starts its audio again after a pause.
    pub fn reset(&self) {
        self.engine.request_reset();
    }

    /// Returns the plugin's parameter set, which an app reads to show the
    /// parameters, and a surface of its own writes through.
    pub fn params(&self) -> &Arc<ParamSet> {
        self.engine.set()
    }

    /// Returns how many faults the instance has had, on every thread: a
    /// panic caught, a call refused, an editor that did not open. The first
    /// of each kind is also written as a line of text.
    pub fn faults(&self) -> u64 {
        self.engine.faults()
    }
}

impl<P: Plugin> Drop for Standalone<P> {
    fn drop(&mut self) {
        // An editor still alive holds the engine too, on this thread, so the
        // last to drop frees the plugin here. Any other holder is an audio
        // side, which may run a block yet, and whose drop must not free the
        // plugin on the audio thread: the instance leaks instead.
        let editors = Rc::strong_count(&self.editors).saturating_sub(1);
        let audio_left = Arc::strong_count(&self.engine) > editors.saturating_add(1);
        if audio_left {
            // Before the release, which reports it.
            self.engine.audio_outlived();
        }
        let free = self.engine.release() == Release::Free;
        // SAFETY: taken once, here, as the instance drops, and never read
        // again.
        let engine = unsafe { ManuallyDrop::take(&mut self.engine) };
        if !free || audio_left {
            let _leaked = Arc::into_raw(engine);
        } else {
            // The plugin drops here when no editor holds it any more, inside
            // a guard of the process's own, since the instance's own goes
            // with it.
            guard((), || drop(engine));
        }
    }
}

/// The audio thread's side of a [`Standalone`]: it plays the app's blocks.
///
/// Thread: audio. It is `Send`, so a device's callback can own it, and it
/// allocates nothing.
///
/// # Examples
///
/// ```
#[doc = include_str!("../doc/gain.rs")]
/// use hapto_host::standalone::Standalone;
///
/// let app = Standalone::<Gain>::new();
/// let mut audio = app.audio();
/// let mut out = [[0.5_f32; 8]; 2];
/// let [left, right] = &mut out;
/// // Before an activation a block is silence.
/// let block = &mut [&mut left[..], &mut right[..]];
/// let played = audio.process(&[], block, 8, &[], |_sent| {});
/// assert_eq!(played, None);
/// assert_eq!(out, [[0.0; 8]; 2]);
/// ```
pub struct StandaloneAudio<P: Plugin> {
    engine: Arc<Engine<P, Link>>,
}

impl<P: Plugin> fmt::Debug for StandaloneAudio<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StandaloneAudio")
            .field("plugin", &P::INFO.name)
            .finish_non_exhaustive()
    }
}

impl<P: Plugin> StandaloneAudio<P> {
    /// Plays one block of `frames` frames from `inputs` into `outputs`, each
    /// a channel, with `midi` as its MIDI in, and returns the processor's
    /// status; `None` for a block that did not play, whose outputs are
    /// silence.
    ///
    /// `midi` is the app's packets for the block, each at a frame of it, MIDI
    /// 1.0 or MIDI 2.0: the engine upgrades MIDI 1.0 as an exporter's MIDI
    /// arrives. They may come in any order, but those at one frame are read
    /// in the order given. A packet at or past the block's end plays
    /// at its last frame, and one past the room
    /// [`ProcessSetup::max_events`](crate::ProcessSetup::max_events) gives is
    /// dropped and counted.
    ///
    /// `sent` is handed what the plugin sent, MIDI 2.0 as it sent it, each
    /// packet at a frame of the block and in the order it sent them, once
    /// the block has played: for an app to send to its devices. It is not
    /// called for a block that did not play.
    ///
    /// The block is `frames` long, so a plugin with no channels plays its
    /// MIDI too, and a channel shorter than that shortens it to the
    /// channel's length. Channels past [`MAX_CHANNELS`] are passed over: an
    /// output among them is left as it was, and an input is not read. The
    /// processor sees the activation's layout: a missing input reads
    /// silence. Two inputs may be one buffer, but inputs that share part of
    /// one are refused, and the block is silence.
    pub fn process(
        &mut self,
        inputs: &[&[f32]],
        outputs: &mut [&mut [f32]],
        frames: usize,
        midi: &[Event],
        sent: impl FnOnce(&[Event]),
    ) -> Option<Status> {
        let ins = &inputs[..inputs.len().min(MAX_CHANNELS)];
        let outs_len = outputs.len().min(MAX_CHANNELS);
        let frames = ins
            .iter()
            .map(|c| c.len())
            .chain(outputs[..outs_len].iter().map(|c| c.len()))
            .fold(frames, usize::min);
        if partly_overlapping(ins, frames) {
            for channel in outputs[..outs_len].iter_mut() {
                channel.fill(0.0);
            }
            return None;
        }
        let mut in_ptrs = [std::ptr::null_mut::<f32>(); MAX_CHANNELS];
        for (ptr, channel) in in_ptrs.iter_mut().zip(ins) {
            *ptr = channel.as_ptr().cast_mut();
        }
        let mut out_ptrs = [std::ptr::null_mut::<f32>(); MAX_CHANNELS];
        for (ptr, channel) in out_ptrs.iter_mut().zip(outputs.iter_mut()) {
            *ptr = channel.as_mut_ptr();
        }
        // SAFETY: each pointer is a channel borrowed for this call and at
        // least `frames` long, so valid for it. The outputs are exclusive
        // borrows, so none is read or written elsewhere meanwhile, and none
        // shares a float with another output or with any input. Two inputs
        // are one buffer or share no float in their first `frames`, as the
        // check above refused any other. The engine writes no input.
        let outcome = unsafe {
            self.engine.process(
                frames,
                &in_ptrs[..ins.len()],
                &out_ptrs[..outs_len],
                |_| {},
                |into| into.extend(midi.iter().copied()),
                sent,
            )
        };
        match outcome {
            Outcome::Played(status) => Some(status),
            // A block of no frames plays nothing, and is silence as a block
            // that did not play is.
            Outcome::Empty | Outcome::Silent => {
                for channel in outputs[..outs_len].iter_mut() {
                    channel.fill(0.0);
                }
                None
            }
        }
    }
}

/// Whether two of `inputs` share part of their first `frames` floats
/// without starting at the same one: the engine takes channels that are one
/// buffer or share nothing.
fn partly_overlapping(inputs: &[&[f32]], frames: usize) -> bool {
    let span = |channel: &&[f32]| {
        let start = channel.as_ptr() as usize;
        (start, start + frames * std::mem::size_of::<f32>())
    };
    inputs.iter().enumerate().any(|(index, a)| {
        let (a_start, a_end) = span(a);
        inputs[..index].iter().any(|b| {
            let (b_start, b_end) = span(b);
            a_start != b_start && a_start < b_end && b_start < a_end
        })
    })
}

/// The plugin's editor, opened in a window the app makes for it.
///
/// It keeps the editor's life as an exporter's view does: the size and scale
/// the app gives it, a close that waits for a call the editor is still in,
/// and a panic caught and counted. Its tick is the instance's: each
/// [`Standalone::tick`] ticks it while open. It closes as it drops.
///
/// Thread: main.
///
/// # Examples
///
/// ```
#[doc = include_str!("../doc/gain.rs")]
/// use hapto_host::standalone::Standalone;
///
/// let app = Standalone::<Gain>::new();
/// // The gain hands out no editor: the app shows a window of its own.
/// assert!(app.editor().is_none());
/// ```
pub struct StandaloneEditor<P: Plugin> {
    /// Each dropped by hand, inside a guard: an editor or a plugin whose
    /// drop panics must not unwind into the app.
    slot: ManuallyDrop<Rc<EditorSlot>>,
    engine: ManuallyDrop<Arc<Engine<P, Link>>>,
    /// Counts it among the instance's editors alive.
    _counted: Rc<()>,
}

impl<P: Plugin> fmt::Debug for StandaloneEditor<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StandaloneEditor")
            .field("plugin", &P::INFO.name)
            .field("slot", &self.slot)
            .finish_non_exhaustive()
    }
}

/// Why an editor did not open.
///
/// # Examples
///
/// ```
/// use hapto_host::standalone::OpenError;
///
/// let error = OpenError::Busy;
/// assert!(error.to_string().contains("in a call"));
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub enum OpenError {
    /// The window gave no handle.
    Window(HandleError),
    /// It was open already.
    Open,
    /// It was still in a call when the app opened it again. A fault of the
    /// instance, written in its report.
    Busy,
    /// The editor refused, with its reason. A fault of the instance, written
    /// in its report.
    Failed(Box<dyn Error>),
    /// The editor panicked inside its open. A fault of the instance.
    Panicked,
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenError::Window(error) => write!(f, "the window gave no handle: {error}"),
            OpenError::Open => f.write_str("the editor is open already"),
            OpenError::Busy => f.write_str("the editor was still in a call"),
            OpenError::Failed(error) => write!(f, "the editor did not open: {error}"),
            OpenError::Panicked => f.write_str("the editor panicked as it opened"),
        }
    }
}

impl Error for OpenError {}

impl<P: Plugin> StandaloneEditor<P> {
    /// Returns the size the app gave last, or the one it opens at.
    pub fn size(&self) -> Size {
        self.slot.size()
    }

    /// Returns the scale the app gave last, 1 until it gives one.
    pub fn scale(&self) -> f64 {
        self.slot.scale()
    }

    /// Opens the editor inside `window`, which the app keeps until it
    /// [closes](Self::close) the editor or drops it.
    ///
    /// # Errors
    ///
    /// [`OpenError`] says why it did not open; the window then shows nothing
    /// of the editor's.
    pub fn open(&self, window: &impl HasWindowHandle) -> Result<(), OpenError> {
        let raw = window.window_handle().map_err(OpenError::Window)?.as_raw();
        // The slot catches a panic in the editor's own open; this catches
        // one after it, as the editor is told what arrived meanwhile.
        self.engine.guard(Err(OpenError::Panicked), || {
            match self.slot.open(&ParentWindow::new(raw)) {
                Opened::Yes => Ok(()),
                Opened::Already => Err(OpenError::Open),
                Opened::Busy => {
                    self.engine.editor_failed(
                        &"the app opened the editor again while it was still in a call, \
                          so it stays closed until the next open",
                    );
                    Err(OpenError::Busy)
                }
                Opened::Failed(error) => {
                    self.engine.editor_failed(&error);
                    Err(OpenError::Failed(error))
                }
                Opened::Panicked => {
                    self.engine.editor_panicked();
                    Err(OpenError::Panicked)
                }
            }
        })
    }

    /// Closes the editor, now or once the call it is in returns.
    pub fn close(&self) {
        self.engine.guard((), || self.slot.close());
    }

    /// Gives the editor the size of its window, in logical pixels.
    pub fn set_size(&self, size: Size) {
        self.engine.guard((), || self.slot.set_size(size));
    }

    /// Gives the editor its window's scale: 1 at the system's base density.
    /// A scale that is not a positive finite number is passed over, as a
    /// VST3 view refuses one.
    pub fn set_scale(&self, scale: f64) {
        self.engine.guard((), || {
            self.slot.set_scale(scale);
        });
    }

    /// Whether it is open, or opening.
    pub fn is_open(&self) -> bool {
        self.slot.is_open()
    }
}

impl<P: Plugin> Drop for StandaloneEditor<P> {
    fn drop(&mut self) {
        self.close();
        // SAFETY: each taken once, here, as the editor drops, and never read
        // again.
        let (slot, engine) = unsafe {
            (
                ManuallyDrop::take(&mut self.slot),
                ManuallyDrop::take(&mut self.engine),
            )
        };
        // The editor drops with its slot, a fault of the instance's if it
        // panics. The plugin drops with the engine when this editor is the
        // last to hold it, the app having dropped its instance: inside a
        // guard of the process's own, since the instance's goes with it.
        engine.guard((), || drop(slot));
        guard((), || drop(engine));
    }
}
