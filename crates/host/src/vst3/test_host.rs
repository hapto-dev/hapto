//! A VST3 host inside the test's own process: the host's side (streams,
//! parameter queues, the component handler) in Rust, driving the plugin
//! through the exporter's own vtables.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::marker::PhantomData;
use std::time::{Duration, Instant};

use ::vst3::Steinberg::Vst::{
    AudioBusBuffers, AudioBusBuffers__type0, BusDirection, BusDirections_, DataEvent, Event,
    IAudioProcessor, IAudioProcessorTrait, IComponent, IComponentHandler, IComponentHandler2,
    IComponentHandler2Trait, IComponentHandlerTrait, IComponentTrait, IEditController,
    IEditControllerTrait, IEventList, IEventListTrait, IMidiMapping, IMidiMappingTrait,
    INoteExpressionController, INoteExpressionControllerTrait, IParamValueQueue,
    IParamValueQueueTrait, IParameterChanges, IParameterChangesTrait, LegacyMIDICCOutEvent,
    MediaTypes_, NoteExpressionTypeInfo, NoteExpressionValueEvent, NoteOffEvent, NoteOnEvent,
    ParamID, ParamValue, ParameterInfo, ParameterInfo_::ParameterFlags_, PolyPressureEvent,
    ProcessData, ProcessModes_, ProcessSetup as VstSetup, SpeakerArr, SpeakerArrangement,
    String128, SymbolicSampleSizes_,
};
use ::vst3::Steinberg::{
    FIDString, IBStream, IBStreamTrait, IPluginBaseTrait, TBool, int32, int64, kInternalError,
    kInvalidArgument, kNoInterface, kNotImplemented, kNotInitialized, kOutOfMemory, kResultFalse,
    kResultOk, kResultTrue, tresult,
};
use ::vst3::{Class, ComPtr, ComWrapper};

use super::component::Component;
use super::events::{
    DATA, EXPRESSION, LEGACY_CC_OUT, NOTE_OFF, NOTE_ON, POLY_PRESSURE, SYSEX, blank,
};
use super::text::copy_utf16;
use crate::Plugin;
use crate::engine::timer::pump;
use crate::layout::Width;

#[cfg(test)]
mod run_loop;
mod view;
mod window;

#[cfg(test)]
pub(crate) use run_loop::TestRunLoop;
pub use view::TestView;
pub use window::{ChildWindow, HiddenWindow};

/// VST3's restart flag asking the host to re-read every parameter value.
pub const RESTART_VALUES: i32 = 4;
/// VST3's restart flag saying the latency changed.
pub const RESTART_LATENCY: i32 = 8;

/// What the plugin told the host through its component handler.
///
/// # Examples
///
/// ```
#[doc = include_str!("../../doc/gain.rs")]
/// use hapto_host::vst3::{HostCall, TestHost};
///
/// let host = TestHost::<Gain>::new();
/// let id = host.parameter(0).expect("listed").id;
///
/// // The plugin's UI drags the gain to +6 dB.
/// EDIT.store(true, Ordering::Relaxed);
/// host.tick();
/// // The host hears the whole gesture, the value as 0..1.
/// assert_eq!(
///     host.calls(),
///     [HostCall::Begin(id), HostCall::Perform(id, 0.625), HostCall::End(id)]
/// );
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HostCall {
    /// `beginEdit`, with the parameter's host id.
    Begin(u32),
    /// `performEdit`: the host id and the 0..1 value.
    Perform(u32, f64),
    /// `endEdit`.
    End(u32),
    /// `restartComponent`, with its flags ([`RESTART_VALUES`],
    /// [`RESTART_LATENCY`]).
    Restart(i32),
    /// `IComponentHandler2::setDirty`, with its state: state the host does
    /// not list changed, so its project counts as changed.
    Dirty(bool),
}

/// A call the plugin refused, and the `tresult` it answered: anything but
/// `kResultOk`.
///
/// # Examples
///
/// ```
#[doc = include_str!("../../doc/gain.rs")]
/// use hapto_host::vst3::{Refused, TestHost};
///
/// let host = TestHost::<Gain>::new();
/// // A block that is not a saved one: the host is told the load failed.
/// let refused: Refused = host.load(b"not a block").expect_err("refused");
/// assert_eq!(refused.to_string(), "the plugin answered kResultFalse");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refused(pub i32);

impl Refused {
    /// `Ok` for `kResultOk`, and the refusal for any other answer.
    fn check(result: tresult) -> Result<(), Refused> {
        if result == kResultOk {
            Ok(())
        } else {
            Err(Refused(result))
        }
    }
}

/// The SDK's names for the answers a plugin refuses a call with.
const REFUSALS: [(tresult, &str); 7] = [
    (kResultFalse, "kResultFalse"),
    (kInvalidArgument, "kInvalidArgument"),
    (kNotImplemented, "kNotImplemented"),
    (kInternalError, "kInternalError"),
    (kNotInitialized, "kNotInitialized"),
    (kOutOfMemory, "kOutOfMemory"),
    (kNoInterface, "kNoInterface"),
];

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match REFUSALS.iter().find(|(result, _)| *result == self.0) {
            Some((_, name)) => write!(f, "the plugin answered {name}"),
            None => write!(f, "the plugin answered the tresult {}", self.0),
        }
    }
}

impl std::error::Error for Refused {}

/// A parameter as the host lists it.
///
/// # Examples
///
/// ```
#[doc = include_str!("../../doc/gain.rs")]
/// use hapto_host::vst3::TestHost;
///
/// let host = TestHost::<Gain>::new();
/// let info = host.parameter(0).expect("listed");
/// assert_eq!((info.title.as_str(), info.units.as_str()), ("Gain", "dB"));
/// assert_eq!(info.step_count, 0, "continuous");
/// assert_eq!(info.default_normalized, 0.5, "0 dB, halfway through -24..24");
/// assert!(info.can_automate && !info.is_list && !info.is_bypass);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct ParamInfo {
    /// The host id.
    pub id: u32,
    /// The name.
    pub title: String,
    /// The unit.
    pub units: String,
    /// VST3's step count: 0 for continuous, else the values minus one.
    pub step_count: i32,
    /// The default, as 0..1.
    pub default_normalized: f64,
    /// Whether the host may automate it.
    pub can_automate: bool,
    /// Whether the host shows it as a list.
    pub is_list: bool,
    /// Whether it is the bypass.
    pub is_bypass: bool,
    /// Whether the host shows it nowhere, as the proxies a plugin's MIDI
    /// controllers reach it through.
    pub is_hidden: bool,
}

/// How a [`TestHost`] makes its instance.
///
/// # Examples
///
/// ```
#[doc = include_str!("../../doc/gain.rs")]
/// use hapto_host::vst3::{Options, TestHost};
///
/// // A host that creates the instance on another thread, as some do.
/// let host = TestHost::<Gain>::with(Options {
///     created_elsewhere: true,
///     ..Options::default()
/// });
/// assert_eq!(host.parameter_count(), 1);
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Create it on another thread, as a host may; it is initialized on the
    /// test's thread all the same.
    pub created_elsewhere: bool,
    /// Start its own timer, as in a host, which only
    /// [`pump_until`](TestHost::pump_until) runs.
    pub timer: bool,
}

/// A VST3 host for tests, holding one instance of `P`.
///
/// It initializes the instance on the test's thread, which becomes the
/// instance's main thread, and makes every call through the component's COM
/// interfaces. [`tick`](Self::tick) stands in for the exporter's timer, or a
/// host made [`with_timer`](Self::with_timer) runs the real one.
///
/// Processing takes the host's buffers either separate or in place, after a
/// list of `(host id, 0..1)` changes, each sent as one queue of one point at
/// the block's start, or [queues of points at sample
/// offsets](Self::process_queues). A block of up to 16 channels and 64
/// queues of up to 64 points allocates nothing in the host.
///
/// Its instance must have no fault, on any thread: it checks when dropped,
/// allowing only the faults a test provokes on purpose and declares with
/// [`expect_faults`](Self::expect_faults).
///
/// # Examples
///
/// ```
#[doc = include_str!("../../doc/gain.rs")]
/// use hapto_host::vst3::TestHost;
///
/// let mut host = TestHost::<Gain>::new();
/// let id = host.parameter(0).expect("listed").id;
/// host.setup(48_000.0, 4)?;
/// host.activate()?;
/// host.start()?;
///
/// let (mut left, mut right) = ([1.0_f32; 4], [1.0_f32; 4]);
/// // The host sends 0.0 for the gain with the block: -24 dB.
/// let block = &mut [&mut left[..], &mut right[..]];
/// host.process_in_place(block, 4, &[(id, 0.0)])?;
/// assert!((left[0] - 0.0631).abs() < 1e-4);
/// # Ok::<(), hapto_host::vst3::Refused>(())
/// ```
pub struct TestHost<P: Plugin> {
    component: ComWrapper<Component<P>>,
    plugin: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
    controller: ComPtr<IEditController>,
    handler: ComWrapper<Recorder>,
    blocks: Blocks,
    /// The faults the test provokes on purpose.
    provoked: Cell<u64>,
}

impl<P: Plugin> std::fmt::Debug for TestHost<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestHost")
            .field("plugin", &P::INFO.name)
            .finish_non_exhaustive()
    }
}

impl<P: Plugin> Default for TestHost<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Plugin> TestHost<P> {
    /// Creates and initializes an instance, and gives it a component
    /// handler that records what it is told.
    ///
    /// # Panics
    ///
    /// When the plugin's parameter set has its host event drain taken: the
    /// exporter owns it.
    pub fn new() -> Self {
        Self::initialize(Component::<P>::wrap(false))
    }

    /// Creates and initializes an instance whose own timer ticks it, as in a
    /// host.
    ///
    /// The timer ticks on this thread's message loop (Windows) or run loop
    /// (macOS), which only [`pump_until`](Self::pump_until) runs. No timer
    /// exists on any other platform.
    ///
    /// # Panics
    ///
    /// As [`new`](Self::new).
    pub fn with_timer() -> Self {
        Self::with(Options {
            timer: true,
            ..Options::default()
        })
    }

    /// Creates and initializes an instance as `options` say.
    ///
    /// # Panics
    ///
    /// As [`new`](Self::new).
    pub fn with(options: Options) -> Self {
        let timer = options.timer;
        let make = move || Component::<P>::wrap(timer);
        let component = if options.created_elsewhere {
            std::thread::spawn(make)
                .join()
                .expect("created on another thread")
        } else {
            make()
        };
        Self::initialize(component)
    }

    /// Runs this thread's message loop or run loop, as a host's would,
    /// until `done` holds or ten seconds pass, and returns whether it holds:
    /// a host made [`with_timer`](Self::with_timer) ticks meanwhile.
    pub fn pump_until(&self, mut done: impl FnMut() -> bool) -> bool {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            if done() {
                return true;
            }
            pump(Duration::from_millis(10));
        }
        done()
    }

    /// Creates an instance on another thread, as a host may, then
    /// initializes it on this one, which becomes its main thread.
    ///
    /// # Panics
    ///
    /// As [`new`](Self::new).
    pub fn created_elsewhere() -> Self {
        Self::with(Options {
            created_elsewhere: true,
            ..Options::default()
        })
    }

    fn initialize(component: ComWrapper<Component<P>>) -> Self {
        let plugin = component.to_com_ptr::<IComponent>().expect("a component");
        let processor = component
            .to_com_ptr::<IAudioProcessor>()
            .expect("an audio processor");
        let controller = component
            .to_com_ptr::<IEditController>()
            .expect("its own controller");
        let handler = ComWrapper::new(Recorder::default());
        // SAFETY: the component was just made and these interfaces hold it.
        // `initialize` takes a null context; the handler is a live
        // `IComponentHandler`, and the component keeps its own reference.
        unsafe {
            assert_eq!(plugin.initialize(std::ptr::null_mut()), kResultOk);
            let recorder = handler
                .as_com_ref::<IComponentHandler>()
                .expect("a handler")
                .as_ptr();
            assert_eq!(controller.setComponentHandler(recorder), kResultOk);
        }
        TestHost {
            blocks: Blocks::new(plugin.clone(), processor.clone()),
            component,
            plugin,
            processor,
            controller,
            handler,
            provoked: Cell::new(0),
        }
    }

    /// Returns how many faults the instance has had, on every thread.
    pub fn faults(&self) -> u64 {
        self.component.faults()
    }

    /// Declares `count` more faults the test provokes on any thread on
    /// purpose, which the check at drop then allows.
    pub fn expect_faults(&self, count: u64) {
        self.provoked.set(self.provoked.get() + count);
    }

    /// The component, for the exporter's own tests.
    #[cfg(test)]
    pub(crate) fn component(&self) -> &Component<P> {
        &self.component
    }

    /// The component's interfaces, for the exporter's own tests.
    #[cfg(test)]
    pub(crate) fn interfaces(
        &self,
    ) -> (
        &ComPtr<IComponent>,
        &ComPtr<IAudioProcessor>,
        &ComPtr<IEditController>,
    ) {
        (&self.plugin, &self.processor, &self.controller)
    }

    /// Returns the parameters a host shows, in its order: the plugin's own,
    /// without the hidden proxies the exporter lists for each MIDI 1.0
    /// controller of a plugin that takes MIDI, 2,080 of them, which
    /// [`parameter_count`](Self::parameter_count) counts.
    pub fn parameters(&self) -> Vec<ParamInfo> {
        (0..self.parameter_count())
            .filter_map(|index| self.parameter(index))
            .filter(|info| !info.is_hidden)
            .collect()
    }

    /// Returns how many parameters the host lists, the hidden ones among
    /// them.
    pub fn parameter_count(&self) -> usize {
        // SAFETY: the component is alive while this host holds its
        // interfaces, and the call passes no pointer.
        usize::try_from(unsafe { self.controller.getParameterCount() }).unwrap_or(0)
    }

    /// Returns the parameter the host lists at `index`.
    pub fn parameter(&self, index: usize) -> Option<ParamInfo> {
        // SAFETY: all zeros is a valid info, which the call overwrites.
        let mut info: ParameterInfo = unsafe { std::mem::zeroed() };
        let index = int32::try_from(index).ok()?;
        // SAFETY: the component is alive while this host holds its
        // interfaces, and `info` is valid for writing one `ParameterInfo`
        // during the call.
        if unsafe { self.controller.getParameterInfo(index, &mut info) } != kResultOk {
            return None;
        }
        Some(ParamInfo {
            id: info.id,
            title: wide(&info.title),
            units: wide(&info.units),
            step_count: info.stepCount,
            default_normalized: info.defaultNormalizedValue,
            can_automate: info.flags & ParameterFlags_::kCanAutomate != 0,
            is_list: info.flags & ParameterFlags_::kIsList != 0,
            is_bypass: info.flags & ParameterFlags_::kIsBypass != 0,
            is_hidden: info.flags & ParameterFlags_::kIsHidden != 0,
        })
    }

    /// Returns the text the plugin shows for a 0..1 value.
    pub fn text(&self, id: u32, normalized: f64) -> Option<String> {
        let mut text: String128 = [0; 128];
        // SAFETY: the component is alive while this host holds its
        // interfaces, and `text` is valid for writing one `String128` during
        // the call.
        let result = unsafe {
            self.controller
                .getParamStringByValue(id, normalized, &mut text)
        };
        (result == kResultOk).then(|| wide(&text))
    }

    /// Returns the 0..1 value the plugin reads from text.
    pub fn parse(&self, id: u32, text: &str) -> Option<f64> {
        let mut wide_text: String128 = [0; 128];
        copy_utf16(text, &mut wide_text);
        let mut normalized = 0.0;
        // SAFETY: the component is alive while this host holds its interfaces;
        // `wide_text` is a terminated `String128` and `normalized` is valid
        // for writing one value, both during the call.
        let result = unsafe {
            self.controller
                .getParamValueByString(id, wide_text.as_mut_ptr(), &mut normalized)
        };
        (result == kResultOk).then_some(normalized)
    }

    /// Returns the real value for the plugin's 0..1.
    pub fn plain(&self, id: u32, normalized: f64) -> f64 {
        // SAFETY: the component is alive while this host holds its
        // interfaces, and the call passes no pointer.
        unsafe { self.controller.normalizedParamToPlain(id, normalized) }
    }

    /// Returns the plugin's 0..1 for a real value.
    pub fn normalize(&self, id: u32, plain: f64) -> f64 {
        // SAFETY: the component is alive while this host holds its
        // interfaces, and the call passes no pointer.
        unsafe { self.controller.plainParamToNormalized(id, plain) }
    }

    /// Returns the parameter's current 0..1.
    pub fn normalized(&self, id: u32) -> f64 {
        // SAFETY: the component is alive while this host holds its
        // interfaces, and the call passes no pointer.
        unsafe { self.controller.getParamNormalized(id) }
    }

    /// Sets a parameter as a host's own controller would, through
    /// `setParamNormalized`.
    ///
    /// # Errors
    ///
    /// The [`Refused`] answer, for an id the plugin does not list say.
    pub fn set_normalized(&self, id: u32, normalized: f64) -> Result<(), Refused> {
        // SAFETY: the component is alive while this host holds its
        // interfaces, and the call passes no pointer.
        Refused::check(unsafe { self.controller.setParamNormalized(id, normalized) })
    }

    /// Asks for a main bus layout through `setBusArrangements`.
    ///
    /// # Errors
    ///
    /// The [`Refused`] answer, for a layout the plugin does not declare, or
    /// while it is active.
    pub fn arrange(&self, layout: Width) -> Result<(), Refused> {
        let mut input = arrangement(layout.inputs);
        let mut output = arrangement(layout.outputs);
        let input_count = int32::from(layout.inputs > 0);
        // SAFETY: the component is alive while this host holds its
        // interfaces, and `input` and `output` are one arrangement each, as
        // the counts say.
        Refused::check(unsafe {
            self.processor
                .setBusArrangements(&mut input, input_count, &mut output, 1)
        })
    }

    /// Returns the main bus layout, read through `getBusArrangement`.
    pub fn layout(&self) -> Width {
        let count = |direction| {
            let mut arrangement: SpeakerArrangement = 0;
            // SAFETY: the component is alive while this host holds its
            // interfaces, and `arrangement` is valid for writing one
            // arrangement.
            let result = unsafe {
                self.processor
                    .getBusArrangement(direction, 0, &mut arrangement)
            };
            if result == kResultOk {
                arrangement.count_ones() as usize
            } else {
                0
            }
        };
        Width::new(
            count(BusDirections_::kInput as BusDirection),
            count(BusDirections_::kOutput as BusDirection),
        )
    }

    /// Calls `setupProcessing`, realtime and 32-bit.
    ///
    /// # Errors
    ///
    /// The [`Refused`] answer, for a rate that is not a positive number or
    /// while the plugin is active, say.
    pub fn setup(&self, sample_rate: f64, max_frames: usize) -> Result<(), Refused> {
        let mut setup = VstSetup {
            processMode: ProcessModes_::kRealtime as int32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
            maxSamplesPerBlock: int32::try_from(max_frames).unwrap_or(int32::MAX),
            sampleRate: sample_rate,
        };
        // SAFETY: the component is alive while this host holds its
        // interfaces, and `setup` is valid for reading during the call.
        Refused::check(unsafe { self.processor.setupProcessing(&mut setup) })
    }

    /// Calls `setActive(true)`.
    ///
    /// # Errors
    ///
    /// The [`Refused`] answer.
    pub fn activate(&self) -> Result<(), Refused> {
        // SAFETY: the component is alive while this host holds its
        // interfaces, and the call passes no pointer.
        Refused::check(unsafe { self.plugin.setActive(1) })
    }

    /// Calls `setActive(false)`.
    ///
    /// # Errors
    ///
    /// The [`Refused`] answer.
    pub fn deactivate(&self) -> Result<(), Refused> {
        // SAFETY: the component is alive while this host holds its
        // interfaces, and the call passes no pointer.
        Refused::check(unsafe { self.plugin.setActive(0) })
    }

    /// Calls `setProcessing(true)`.
    ///
    /// # Errors
    ///
    /// The [`Refused`] answer.
    pub fn start(&self) -> Result<(), Refused> {
        // SAFETY: the component is alive while this host holds its
        // interfaces, and the call passes no pointer.
        Refused::check(unsafe { self.processor.setProcessing(1) })
    }

    /// Calls `setProcessing(false)`.
    ///
    /// # Errors
    ///
    /// The [`Refused`] answer.
    pub fn stop(&self) -> Result<(), Refused> {
        // SAFETY: the component is alive while this host holds its
        // interfaces, and the call passes no pointer.
        Refused::check(unsafe { self.processor.setProcessing(0) })
    }

    /// Processes `frames` frames from separate input buffers into separate
    /// output buffers, after `changes`.
    ///
    /// # Errors
    ///
    /// The [`Refused`] answer to `process`.
    ///
    /// # Panics
    ///
    /// If a buffer is shorter than `frames`.
    pub fn process(
        &mut self,
        inputs: &[&[f32]],
        outputs: &mut [&mut [f32]],
        frames: usize,
        changes: &[(u32, f64)],
    ) -> Result<(), Refused> {
        self.blocks.process(inputs, outputs, frames, changes)
    }

    /// Processes `frames` frames in place: the same buffers in and out, as
    /// many hosts hand them.
    ///
    /// # Errors
    ///
    /// The [`Refused`] answer to `process`.
    ///
    /// # Panics
    ///
    /// If a buffer is shorter than `frames`.
    pub fn process_in_place(
        &mut self,
        channels: &mut [&mut [f32]],
        frames: usize,
        changes: &[(u32, f64)],
    ) -> Result<(), Refused> {
        self.blocks.process_in_place(channels, frames, changes)
    }

    /// Processes `frames` frames in place after `queues`, each a host id
    /// and its points as `(sample offset, 0..1)`, sent as the host sends
    /// automation.
    ///
    /// # Errors
    ///
    /// The [`Refused`] answer to `process`.
    ///
    /// # Panics
    ///
    /// If a buffer is shorter than `frames`.
    ///
    /// # Examples
    ///
    /// ```
    #[doc = include_str!("../../doc/gain.rs")]
    /// use hapto_host::vst3::TestHost;
    ///
    /// let mut host = TestHost::<Gain>::new();
    /// let id = host.parameter(0).expect("listed").id;
    /// host.setup(48_000.0, 4)?;
    /// host.activate()?;
    /// host.start()?;
    /// let (mut left, mut right) = ([1.0_f32; 4], [1.0_f32; 4]);
    /// // Two points in the block: the processor sees the last for the whole
    /// // block.
    /// let queues: [(u32, &[(i32, f64)]); 1] = [(id, &[(0, 0.5), (2, 0.0)])];
    /// let block = &mut [&mut left[..], &mut right[..]];
    /// host.process_queues(block, 4, &queues)?;
    /// let first = left[0];
    /// assert!((first - 0.0631).abs() < 1e-4, "-24 dB from the first sample");
    /// assert_eq!(host.normalized(id), 0.0);
    /// # Ok::<(), hapto_host::vst3::Refused>(())
    /// ```
    pub fn process_queues(
        &mut self,
        channels: &mut [&mut [f32]],
        frames: usize,
        queues: &[(u32, &[(i32, f64)])],
    ) -> Result<(), Refused> {
        self.blocks.process_queues(channels, frames, queues)
    }

    /// Processes `frames` frames in place with `events` on the event bus,
    /// after `queues`, as [`process_queues`](Self::process_queues) sends
    /// them: a host sends a MIDI controller through the proxy that
    /// [`midi_mapping`](Self::midi_mapping) names.
    ///
    /// # Errors
    ///
    /// The [`Refused`] answer to `process`.
    ///
    /// # Panics
    ///
    /// If a buffer is shorter than `frames`.
    pub fn process_events(
        &mut self,
        channels: &mut [&mut [f32]],
        frames: usize,
        events: &[BusEvent<'_>],
        queues: &[(u32, &[(i32, f64)])],
    ) -> Result<(), Refused> {
        self.blocks.process_events(channels, frames, events, queues)
    }

    /// Processes `frames` frames in place with `queue` alone, which claims
    /// `claimed` points however many it holds, as a broken host's: a read
    /// past the points it holds fails. Answers how many points the plugin
    /// asked for.
    ///
    /// # Errors
    ///
    /// The [`Refused`] answer to `process`.
    ///
    /// # Panics
    ///
    /// If a buffer is shorter than `frames`.
    pub fn process_claiming(
        &mut self,
        channels: &mut [&mut [f32]],
        frames: usize,
        queue: (u32, &[(i32, f64)]),
        claimed: i32,
    ) -> Result<usize, Refused> {
        self.blocks
            .process_claiming(channels, frames, queue, claimed)
    }

    /// Returns the proxy parameter a MIDI 1.0 controller on `channel`, 0 to
    /// 15, reaches the plugin through, as a host asks `IMidiMapping`:
    /// controllers 0 to 127, then 128 for channel pressure and 129 for the
    /// pitch bend, as VST3 numbers them. `None` for a plugin that takes no
    /// MIDI.
    pub fn midi_mapping(&self, channel: i16, controller: i16) -> Option<u32> {
        let mapping = self.controller.cast::<IMidiMapping>()?;
        let mut id = 0;
        // SAFETY: the component is alive while this host holds its
        // interfaces, and `id` is valid for writing one id during the call.
        let found = unsafe { mapping.getMidiControllerAssignment(0, channel, controller, &mut id) };
        (found == kResultTrue).then_some(id)
    }

    /// Returns what the plugin sent in the last block, as the host's output
    /// event list took it. A SysEx's bytes are the plugin's, which it keeps
    /// until its next block, so this borrows the host until then.
    pub fn sent(&mut self) -> Vec<BusEvent<'_>> {
        let list = self.blocks.sent.list.borrow();
        list.iter()
            // SAFETY: a data event's bytes are in the component's room for
            // the block's SysEx, which it clears at its next block alone;
            // `&mut self` keeps any block from running, here or on an audio
            // side, while the events borrow it.
            .filter_map(|event| unsafe { BusEvent::from_vst(event) })
            .collect()
    }

    /// Returns the note expressions the plugin offers on `channel`, each its
    /// VST3 type and its title.
    pub fn note_expressions(&self, channel: i16) -> Vec<(u32, String)> {
        let Some(expressions) = self.controller.cast::<INoteExpressionController>() else {
            return Vec::new();
        };
        // SAFETY: the component is alive while this host holds its
        // interfaces, and the call passes no pointer.
        let count = unsafe { expressions.getNoteExpressionCount(0, channel) };
        (0..count)
            .filter_map(|index| {
                // SAFETY: all zeros is a valid info, which the call
                // overwrites.
                let mut info: NoteExpressionTypeInfo = unsafe { std::mem::zeroed() };
                // SAFETY: as above; `info` is valid for writing one info
                // during the call.
                let read =
                    unsafe { expressions.getNoteExpressionInfo(0, channel, index, &mut info) };
                (read == kResultOk).then(|| (info.typeId, wide(&info.title)))
            })
            .collect()
    }

    /// Returns the text the plugin shows for note expression `kind`'s
    /// `value` on `channel`, or `None` where it refuses.
    pub fn expression_text(&self, channel: i16, kind: u32, value: f64) -> Option<String> {
        let expressions = self.controller.cast::<INoteExpressionController>()?;
        let mut text: String128 = [0; 128];
        // SAFETY: the component is alive while this host holds its
        // interfaces, and `text` is valid for writing one `String128` during
        // the call.
        let result = unsafe {
            expressions.getNoteExpressionStringByValue(0, channel, kind, value, &mut text)
        };
        (result == kResultOk).then(|| wide(&text))
    }

    /// Returns the value the plugin reads from `text` for note expression
    /// `kind` on `channel`, or `None` where it refuses.
    pub fn expression_value(&self, channel: i16, kind: u32, text: &str) -> Option<f64> {
        let expressions = self.controller.cast::<INoteExpressionController>()?;
        let mut wide_text: String128 = [0; 128];
        copy_utf16(text, &mut wide_text);
        let mut value = 0.0;
        // SAFETY: the component is alive while this host holds its
        // interfaces; `wide_text` is a terminated `String128` and `value` is
        // valid for writing one value, both during the call.
        let result = unsafe {
            expressions.getNoteExpressionValueByString(
                0,
                channel,
                kind,
                wide_text.as_ptr(),
                &mut value,
            )
        };
        (result == kResultOk).then_some(value)
    }

    /// Returns the output bus's silence flags after the last block: a bit per
    /// channel the plugin says is silent.
    pub fn silence_flags(&self) -> u64 {
        self.blocks.silence
    }

    /// Returns a side of this host for its audio thread, which processes
    /// blocks on a thread of its own while this one goes on.
    pub fn audio_side(&self) -> AudioSide<'_> {
        AudioSide {
            blocks: Blocks::new(self.plugin.clone(), self.processor.clone()),
            host: PhantomData,
        }
    }

    /// Returns the saved block, through `getState`.
    ///
    /// # Errors
    ///
    /// The [`Refused`] answer `getState` gave.
    #[expect(
        clippy::missing_panics_doc,
        reason = "the stream's class lists `IBStream`, so asking it for one cannot fail"
    )]
    pub fn save(&self) -> Result<Vec<u8>, Refused> {
        let stream = ComWrapper::new(Stream::default());
        let pointer = stream.as_com_ref::<IBStream>().expect("a stream").as_ptr();
        // SAFETY: the component is alive while this host holds its
        // interfaces, and `pointer` is a live `IBStream` until `stream`
        // drops, after the call.
        let result = unsafe { self.plugin.getState(pointer) };
        Refused::check(result).map(|()| stream.bytes.borrow().clone())
    }

    /// Loads a block through `setState`, then hands the controller the same
    /// block, as a host does.
    ///
    /// # Errors
    ///
    /// The [`Refused`] answer of the first of the two calls that refused,
    /// for a block that is not a saved one say.
    #[expect(
        clippy::missing_panics_doc,
        reason = "the stream's class lists `IBStream`, so asking it for one cannot fail"
    )]
    pub fn load(&self, bytes: &[u8]) -> Result<(), Refused> {
        let stream = ComWrapper::new(Stream::default());
        stream.bytes.borrow_mut().extend_from_slice(bytes);
        let pointer = stream.as_com_ref::<IBStream>().expect("a stream").as_ptr();
        // SAFETY: the component is alive while this host holds its
        // interfaces, and `pointer` is a live `IBStream` until `stream`
        // drops, after both calls.
        unsafe {
            Refused::check(self.plugin.setState(pointer))?;
            stream.position.set(0);
            Refused::check(self.controller.setComponentState(pointer))
        }
    }

    /// Runs one main-thread tick, as the exporter's timer runs it.
    pub fn tick(&self) {
        self.component.tick();
    }

    /// Returns what the plugin told the host since the last call.
    pub fn calls(&self) -> Vec<HostCall> {
        std::mem::take(&mut *self.handler.calls.borrow_mut())
    }

    /// Returns `getLatencySamples`.
    pub fn latency(&self) -> u32 {
        // SAFETY: the component is alive while this host holds its
        // interfaces, and the call passes no pointer.
        unsafe { self.processor.getLatencySamples() }
    }

    /// Returns `getTailSamples`.
    pub fn tail(&self) -> u32 {
        // SAFETY: the component is alive while this host holds its
        // interfaces, and the call passes no pointer.
        unsafe { self.processor.getTailSamples() }
    }
}

impl<P: Plugin> Drop for TestHost<P> {
    fn drop(&mut self) {
        // SAFETY: the component is alive while this host holds its
        // interfaces, and the call passes no pointer.
        unsafe { self.plugin.terminate() };
        if !std::thread::panicking() {
            assert_eq!(
                self.faults(),
                self.provoked.get(),
                "faults at the boundary that the test did not provoke"
            );
        }
    }
}

/// One side's way to send blocks: its own parameter changes and pointer
/// lists, so the host's main thread and its audio side each have theirs.
struct Blocks {
    plugin: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
    changes: ComWrapper<Changes>,
    events: ComWrapper<Events>,
    /// What the plugin sent in the last block.
    sent: ComWrapper<Events>,
    inputs: Vec<*mut f32>,
    outputs: Vec<*mut f32>,
    /// The output bus's silence flags after the last block.
    silence: u64,
}

impl Blocks {
    fn new(plugin: ComPtr<IComponent>, processor: ComPtr<IAudioProcessor>) -> Self {
        Blocks {
            plugin,
            processor,
            changes: ComWrapper::new(Changes::with_queues(64)),
            events: ComWrapper::new(Events::new()),
            sent: ComWrapper::new(Events::new()),
            inputs: Vec::with_capacity(16),
            outputs: Vec::with_capacity(16),
            silence: 0,
        }
    }

    fn process(
        &mut self,
        inputs: &[&[f32]],
        outputs: &mut [&mut [f32]],
        frames: usize,
        changes: &[(u32, f64)],
    ) -> Result<(), Refused> {
        assert!(inputs.iter().all(|channel| channel.len() >= frames));
        assert!(outputs.iter().all(|channel| channel.len() >= frames));
        self.inputs.clear();
        self.inputs
            .extend(inputs.iter().map(|channel| channel.as_ptr().cast_mut()));
        self.outputs.clear();
        self.outputs
            .extend(outputs.iter_mut().map(|channel| channel.as_mut_ptr()));
        self.fill(changes);
        self.send(frames, inputs.len())
    }

    fn process_in_place(
        &mut self,
        channels: &mut [&mut [f32]],
        frames: usize,
        changes: &[(u32, f64)],
    ) -> Result<(), Refused> {
        let inputs = self.in_place(channels, frames);
        self.fill(changes);
        self.send(frames, inputs)
    }

    fn process_queues(
        &mut self,
        channels: &mut [&mut [f32]],
        frames: usize,
        queues: &[(u32, &[(i32, f64)])],
    ) -> Result<(), Refused> {
        let inputs = self.in_place(channels, frames);
        self.changes.fill(queues.iter().copied());
        self.send(frames, inputs)
    }

    fn process_claiming(
        &mut self,
        channels: &mut [&mut [f32]],
        frames: usize,
        queue: (u32, &[(i32, f64)]),
        claimed: int32,
    ) -> Result<usize, Refused> {
        let inputs = self.in_place(channels, frames);
        self.changes.fill([queue].into_iter());
        if let Some(first) = self.changes.queues.borrow().first() {
            first.claimed.set(Some(claimed));
        }
        self.events.clear();
        self.send(frames, inputs)?;
        let queues = self.changes.queues.borrow();
        Ok(queues.first().map_or(0, |queue| queue.reads.get()))
    }

    fn process_events(
        &mut self,
        channels: &mut [&mut [f32]],
        frames: usize,
        events: &[BusEvent<'_>],
        queues: &[(u32, &[(i32, f64)])],
    ) -> Result<(), Refused> {
        let inputs = self.in_place(channels, frames);
        self.changes.fill(queues.iter().copied());
        self.events.fill(events);
        let sent = self.send(frames, inputs);
        // A SysEx's bytes are the caller's, borrowed for this call alone.
        self.events.clear();
        sent
    }

    /// Points both lists at `channels`, the inputs only as many as the
    /// input bus has, and answers how many that is.
    fn in_place(&mut self, channels: &mut [&mut [f32]], frames: usize) -> usize {
        assert!(channels.iter().all(|channel| channel.len() >= frames));
        self.outputs.clear();
        self.outputs
            .extend(channels.iter_mut().map(|channel| channel.as_mut_ptr()));
        self.inputs.clear();
        self.inputs.extend_from_slice(&self.outputs);
        let mut arrangement: SpeakerArrangement = 0;
        // SAFETY: the component is alive while this side holds its
        // interfaces, and `arrangement` is valid for writing one arrangement.
        let result = unsafe {
            self.processor.getBusArrangement(
                BusDirections_::kInput as BusDirection,
                0,
                &mut arrangement,
            )
        };
        let inputs = if result == kResultOk {
            arrangement.count_ones() as usize
        } else {
            0
        };
        let count = self.inputs.len().min(inputs);
        self.inputs.truncate(count);
        count
    }

    fn fill(&self, changes: &[(u32, f64)]) {
        self.changes.fill(
            changes
                .iter()
                .map(|(id, value)| Queued(*id, Values::Start(std::slice::from_ref(value)))),
        );
    }

    fn send(&mut self, frames: usize, input_channels: usize) -> Result<(), Refused> {
        // SAFETY: the component is alive while this side holds its
        // interfaces, and the call passes no pointer.
        let input_buses = unsafe {
            self.plugin.getBusCount(
                MediaTypes_::kAudio as int32,
                BusDirections_::kInput as int32,
            )
        };
        let mut input = AudioBusBuffers {
            numChannels: int32::try_from(input_channels).unwrap_or(0),
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 {
                channelBuffers32: self.inputs.as_mut_ptr(),
            },
        };
        let mut output = AudioBusBuffers {
            numChannels: int32::try_from(self.outputs.len()).unwrap_or(0),
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 {
                channelBuffers32: self.outputs.as_mut_ptr(),
            },
        };
        let changes = self
            .changes
            .as_com_ref::<IParameterChanges>()
            .expect("parameter changes")
            .as_ptr();
        let events = self
            .events
            .as_com_ref::<IEventList>()
            .expect("events")
            .as_ptr();
        self.sent.clear();
        let sent = self
            .sent
            .as_com_ref::<IEventList>()
            .expect("events")
            .as_ptr();
        let mut data = ProcessData {
            processMode: ProcessModes_::kRealtime as int32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
            numSamples: int32::try_from(frames).unwrap_or(int32::MAX),
            numInputs: input_buses,
            numOutputs: 1,
            inputs: &mut input,
            outputs: &mut output,
            inputParameterChanges: changes,
            outputParameterChanges: std::ptr::null_mut(),
            inputEvents: events,
            outputEvents: sent,
            processContext: std::ptr::null_mut(),
        };
        // SAFETY: the component is alive while this side holds its
        // interfaces. `data` and its two buses live through the call. Each
        // bus's channel list holds `numChannels` pointers into buffers the
        // callers above borrow for at least `frames` floats. The changes and
        // their queues are this side's objects, alive throughout, and so are
        // the events, each SysEx's bytes borrowed by the caller for the call.
        let processed = unsafe { self.processor.process(&mut data) };
        self.silence = output.silenceFlags;
        Refused::check(processed)
    }
}

/// A test host's audio thread: it sends blocks, on a thread of its own,
/// while the host's main thread goes on. Made by
/// [`TestHost::audio_side`], and no longer-lived than the host.
///
/// # Examples
///
/// ```
#[doc = include_str!("../../doc/gain.rs")]
/// use hapto_host::vst3::TestHost;
///
/// let host = TestHost::<Gain>::new();
/// let id = host.parameter(0).expect("listed").id;
/// host.setup(48_000.0, 4)?;
/// host.activate()?;
/// host.start()?;
///
/// // The host's audio thread gets a side of its own and sends blocks on it.
/// let mut audio = host.audio_side();
/// std::thread::scope(|scope| {
///     scope.spawn(move || {
///         let (mut left, mut right) = ([1.0_f32; 4], [1.0_f32; 4]);
///         let block = &mut [&mut left[..], &mut right[..]];
///         assert_eq!(audio.process_in_place(block, 4, &[(id, 0.0)]), Ok(()));
///         assert!(left[0] < 0.1);
///     });
/// });
/// # Ok::<(), hapto_host::vst3::Refused>(())
/// ```
pub struct AudioSide<'a> {
    blocks: Blocks,
    host: PhantomData<&'a ()>,
}

// SAFETY: its changes object and its queues are its own, reached through no
// other reference: the component sees them only during a `process` call
// this side makes. Moving the side therefore moves all access to them. Its
// pointer lists are refilled at each block and read only during it. Its
// interfaces are `ComPtr`s to a component that is `Send` and `Sync`.
unsafe impl Send for AudioSide<'_> {}

impl AudioSide<'_> {
    /// As [`TestHost::process`], on this side's thread.
    ///
    /// # Errors
    ///
    /// The [`Refused`] answer to `process`.
    ///
    /// # Panics
    ///
    /// If a buffer is shorter than `frames`.
    pub fn process(
        &mut self,
        inputs: &[&[f32]],
        outputs: &mut [&mut [f32]],
        frames: usize,
        changes: &[(u32, f64)],
    ) -> Result<(), Refused> {
        self.blocks.process(inputs, outputs, frames, changes)
    }

    /// As [`TestHost::process_in_place`], on this side's thread.
    ///
    /// # Errors
    ///
    /// The [`Refused`] answer to `process`.
    ///
    /// # Panics
    ///
    /// If a buffer is shorter than `frames`.
    pub fn process_in_place(
        &mut self,
        channels: &mut [&mut [f32]],
        frames: usize,
        changes: &[(u32, f64)],
    ) -> Result<(), Refused> {
        self.blocks.process_in_place(channels, frames, changes)
    }

    /// As [`TestHost::process_queues`], on this side's thread.
    ///
    /// # Errors
    ///
    /// The [`Refused`] answer to `process`.
    ///
    /// # Panics
    ///
    /// If a buffer is shorter than `frames`.
    pub fn process_queues(
        &mut self,
        channels: &mut [&mut [f32]],
        frames: usize,
        queues: &[(u32, &[(i32, f64)])],
    ) -> Result<(), Refused> {
        self.blocks.process_queues(channels, frames, queues)
    }

    /// Returns the output bus's silence flags after this side's last block.
    pub fn silence_flags(&self) -> u64 {
        self.blocks.silence
    }
}

impl std::fmt::Debug for AudioSide<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioSide").finish_non_exhaustive()
    }
}

fn arrangement(channels: usize) -> SpeakerArrangement {
    match channels {
        0 => SpeakerArr::kEmpty,
        1 => SpeakerArr::kMono,
        2 => SpeakerArr::kStereo,
        _ => SpeakerArr::k51,
    }
}

fn wide(text: &[u16]) -> String {
    let end = text
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(text.len());
    String::from_utf16_lossy(&text[..end])
}

/// The host's component handler: records each call.
#[derive(Default)]
pub(super) struct Recorder {
    pub(super) calls: RefCell<Vec<HostCall>>,
}

impl Class for Recorder {
    type Interfaces = (IComponentHandler, IComponentHandler2);
}

impl IComponentHandler2Trait for Recorder {
    unsafe fn setDirty(&self, state: TBool) -> tresult {
        self.calls.borrow_mut().push(HostCall::Dirty(state != 0));
        kResultOk
    }

    unsafe fn requestOpenEditor(&self, _: FIDString) -> tresult {
        kNotImplemented
    }

    unsafe fn startGroupEdit(&self) -> tresult {
        kNotImplemented
    }

    unsafe fn finishGroupEdit(&self) -> tresult {
        kNotImplemented
    }
}

impl IComponentHandlerTrait for Recorder {
    unsafe fn beginEdit(&self, id: ParamID) -> tresult {
        self.calls.borrow_mut().push(HostCall::Begin(id));
        kResultOk
    }

    unsafe fn performEdit(&self, id: ParamID, normalized: ParamValue) -> tresult {
        self.calls
            .borrow_mut()
            .push(HostCall::Perform(id, normalized));
        kResultOk
    }

    unsafe fn endEdit(&self, id: ParamID) -> tresult {
        self.calls.borrow_mut().push(HostCall::End(id));
        kResultOk
    }

    unsafe fn restartComponent(&self, flags: int32) -> tresult {
        self.calls.borrow_mut().push(HostCall::Restart(flags));
        kResultOk
    }
}

/// The most points one of the host's queues holds without allocating.
const QUEUE_POINTS: usize = 64;

/// One parameter's points for a block.
struct Queue {
    id: Cell<u32>,
    points: RefCell<Vec<(i32, f64)>>,
    /// How many points it says it holds, when a broken host says more.
    claimed: Cell<Option<int32>>,
    /// How many points the plugin asked for.
    reads: Cell<usize>,
}

impl Default for Queue {
    fn default() -> Self {
        Queue {
            id: Cell::new(0),
            points: RefCell::new(Vec::with_capacity(QUEUE_POINTS)),
            claimed: Cell::new(None),
            reads: Cell::new(0),
        }
    }
}

/// A queue to send: a host id and its points, as `(offset, 0..1)`.
struct Queued<'a>(u32, Values<'a>);

/// A queue's points: given with offsets, or one value at the block's start.
enum Values<'a> {
    Placed(&'a [(i32, f64)]),
    Start(&'a [f64]),
}

impl<'a> From<(u32, &'a [(i32, f64)])> for Queued<'a> {
    fn from((id, points): (u32, &'a [(i32, f64)])) -> Self {
        Queued(id, Values::Placed(points))
    }
}

impl Class for Queue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for Queue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id.get()
    }

    unsafe fn getPointCount(&self) -> int32 {
        self.claimed
            .get()
            .unwrap_or_else(|| int32::try_from(self.points.borrow().len()).unwrap_or(int32::MAX))
    }

    unsafe fn getPoint(&self, index: int32, offset: *mut int32, value: *mut ParamValue) -> tresult {
        self.reads.set(self.reads.get().saturating_add(1));
        let points = self.points.borrow();
        let point = usize::try_from(index)
            .ok()
            .and_then(|index| points.get(index));
        let (Some(&(at, normalized)), false, false) = (point, offset.is_null(), value.is_null())
        else {
            return kInvalidArgument;
        };
        // SAFETY: the exporter passes room for the point; both pointers are
        // non-null here.
        unsafe {
            *offset = at;
            *value = normalized;
        }
        kResultOk
    }

    unsafe fn addPoint(&self, _offset: int32, _value: ParamValue, _index: *mut int32) -> tresult {
        kResultFalse
    }
}

/// A block's parameter changes, one queue each.
struct Changes {
    queues: RefCell<Vec<ComWrapper<Queue>>>,
    count: Cell<usize>,
}

impl Changes {
    /// Room for `count` changes a block, so a block of up to that many
    /// allocates nothing.
    fn with_queues(count: usize) -> Self {
        Changes {
            queues: RefCell::new(
                (0..count)
                    .map(|_| ComWrapper::new(Queue::default()))
                    .collect(),
            ),
            count: Cell::new(0),
        }
    }

    fn fill<'a, S: Into<Queued<'a>>>(&self, sends: impl ExactSizeIterator<Item = S>) {
        let count = sends.len();
        let mut queues = self.queues.borrow_mut();
        while queues.len() < count {
            queues.push(ComWrapper::new(Queue::default()));
        }
        for (queue, send) in queues.iter().zip(sends) {
            let Queued(id, points) = send.into();
            queue.id.set(id);
            queue.claimed.set(None);
            queue.reads.set(0);
            let mut held = queue.points.borrow_mut();
            held.clear();
            match points {
                Values::Placed(points) => held.extend_from_slice(points),
                Values::Start(values) => held.extend(values.iter().map(|&value| (0, value))),
            }
        }
        self.count.set(count);
    }
}

impl Class for Changes {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for Changes {
    unsafe fn getParameterCount(&self) -> int32 {
        int32::try_from(self.count.get()).unwrap_or(int32::MAX)
    }

    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        let queues = self.queues.borrow();
        usize::try_from(index)
            .ok()
            .filter(|&index| index < self.count.get())
            .and_then(|index| queues.get(index))
            .and_then(ComWrapper::as_com_ref::<IParamValueQueue>)
            .map_or(std::ptr::null_mut(), |queue| queue.as_ptr())
    }

    unsafe fn addParameterData(
        &self,
        _id: *const ParamID,
        _index: *mut int32,
    ) -> *mut IParamValueQueue {
        std::ptr::null_mut()
    }
}

/// An event a host sends on the plugin's event bus, as VST3 carries it,
/// for [`TestHost::process_events`]. A note's events name the note by the
/// host's `id`, or -1 for none; velocity and pressure run from 0 to 1, and
/// a note-on's tuning is in cents.
///
/// # Examples
///
/// ```
#[doc = include_str!("../../doc/gain.rs")]
/// use hapto_host::vst3::{BusEvent, TestHost};
///
/// let mut host = TestHost::<Gain>::new();
/// host.setup(48_000.0, 4)?;
/// host.activate()?;
/// host.start()?;
/// // Middle C at frame 1, which the host names note 7, and a SysEx.
/// let events = [
///     BusEvent::NoteOn { offset: 1, channel: 0, pitch: 60, velocity: 1.0, tuning: 0.0, id: 7 },
///     BusEvent::SysEx { offset: 2, bytes: &[0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7] },
/// ];
/// let (mut left, mut right) = ([1.0_f32; 4], [1.0_f32; 4]);
/// host.process_events(&mut [&mut left[..], &mut right[..]], 4, &events, &[])?;
/// // The gain takes no MIDI, so it plays on as before.
/// assert_eq!(left, [1.0; 4]);
/// # Ok::<(), hapto_host::vst3::Refused>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BusEvent<'a> {
    /// A note starting.
    NoteOn {
        /// The frame of the block.
        offset: i32,
        /// The channel, 0 to 15.
        channel: i16,
        /// The key, 0 to 127.
        pitch: i16,
        /// From 0 to 1.
        velocity: f32,
        /// In cents from the key.
        tuning: f32,
        /// The host's name for the note.
        id: i32,
    },
    /// A note ending.
    NoteOff {
        /// The frame of the block.
        offset: i32,
        /// The channel, 0 to 15.
        channel: i16,
        /// The key, 0 to 127.
        pitch: i16,
        /// From 0 to 1.
        velocity: f32,
        /// The host's name for the note.
        id: i32,
    },
    /// A note's pressure.
    Pressure {
        /// The frame of the block.
        offset: i32,
        /// The channel, 0 to 15.
        channel: i16,
        /// The key, 0 to 127.
        pitch: i16,
        /// From 0 to 1.
        pressure: f32,
        /// The host's name for the note.
        id: i32,
    },
    /// A note expression: one of VST3's types for a note the host named.
    Expression {
        /// The frame of the block.
        offset: i32,
        /// The host's name for the note.
        id: i32,
        /// VST3's type: 0 the volume, 2 the tuning, 5 the brightness.
        kind: u32,
        /// From 0 to 1, as VST3 normalizes it.
        value: f64,
    },
    /// A whole SysEx message.
    SysEx {
        /// The frame of the block.
        offset: i32,
        /// Its bytes, F0 to F7.
        bytes: &'a [u8],
    },
    /// A MIDI 1.0 controller, channel pressure, pitch bend or program
    /// change, as VST3 carries the plugin's out: a host's input sends
    /// these through the proxies instead.
    Controller {
        /// The frame of the block.
        offset: i32,
        /// The channel, 0 to 15.
        channel: i16,
        /// The controller, 0 to 127, then VST3's numbers: 128 channel
        /// pressure, 129 the bend, 130 a program change.
        controller: u8,
        /// The value, 0 to 127: the bend's low 7 bits.
        value: i8,
        /// The bend's high 7 bits; 0 for any other.
        value2: i8,
    },
}

impl<'a> BusEvent<'a> {
    /// The event VST3 carried, if it is one of these; a SysEx's bytes
    /// borrowed for `'a`.
    ///
    /// # Safety
    ///
    /// A data event's bytes are valid for reading for `'a`.
    unsafe fn from_vst(event: &Event) -> Option<BusEvent<'a>> {
        let offset = event.sampleOffset;
        Some(match event.r#type {
            NOTE_ON => {
                // SAFETY: the type says which of the union's fields is set.
                let on = unsafe { event.__field0.noteOn };
                BusEvent::NoteOn {
                    offset,
                    channel: on.channel,
                    pitch: on.pitch,
                    velocity: on.velocity,
                    tuning: on.tuning,
                    id: on.noteId,
                }
            }
            NOTE_OFF => {
                // SAFETY: the type says which of the union's fields is set.
                let off = unsafe { event.__field0.noteOff };
                BusEvent::NoteOff {
                    offset,
                    channel: off.channel,
                    pitch: off.pitch,
                    velocity: off.velocity,
                    id: off.noteId,
                }
            }
            POLY_PRESSURE => {
                // SAFETY: the type says which of the union's fields is set.
                let pressure = unsafe { event.__field0.polyPressure };
                BusEvent::Pressure {
                    offset,
                    channel: pressure.channel,
                    pitch: pressure.pitch,
                    pressure: pressure.pressure,
                    id: pressure.noteId,
                }
            }
            EXPRESSION => {
                // SAFETY: the type says which of the union's fields is set.
                let value = unsafe { event.__field0.noteExpressionValue };
                BusEvent::Expression {
                    offset,
                    id: value.noteId,
                    kind: value.typeId,
                    value: value.value,
                }
            }
            DATA => {
                // SAFETY: the type says which of the union's fields is set.
                let data = unsafe { event.__field0.data };
                if data.bytes.is_null() {
                    return None;
                }
                // SAFETY: this function's contract.
                let bytes = unsafe { std::slice::from_raw_parts(data.bytes, data.size as usize) };
                BusEvent::SysEx { offset, bytes }
            }
            LEGACY_CC_OUT => {
                // SAFETY: the type says which of the union's fields is set.
                let out = unsafe { event.__field0.midiCCOut };
                BusEvent::Controller {
                    offset,
                    channel: i16::from(out.channel),
                    controller: out.controlNumber,
                    value: out.value,
                    value2: out.value2,
                }
            }
            _ => return None,
        })
    }

    /// The event as VST3 carries it, its bytes borrowed for as long as
    /// `self`'s are.
    fn to_vst(self) -> Event {
        let mut event = blank(0, 0);
        match self {
            BusEvent::NoteOn {
                offset,
                channel,
                pitch,
                velocity,
                tuning,
                id,
            } => {
                event.sampleOffset = offset;
                event.r#type = NOTE_ON;
                event.__field0.noteOn = NoteOnEvent {
                    channel,
                    pitch,
                    tuning,
                    velocity,
                    length: 0,
                    noteId: id,
                };
            }
            BusEvent::NoteOff {
                offset,
                channel,
                pitch,
                velocity,
                id,
            } => {
                event.sampleOffset = offset;
                event.r#type = NOTE_OFF;
                event.__field0.noteOff = NoteOffEvent {
                    channel,
                    pitch,
                    velocity,
                    noteId: id,
                    tuning: 0.0,
                };
            }
            BusEvent::Pressure {
                offset,
                channel,
                pitch,
                pressure,
                id,
            } => {
                event.sampleOffset = offset;
                event.r#type = POLY_PRESSURE;
                event.__field0.polyPressure = PolyPressureEvent {
                    channel,
                    pitch,
                    pressure,
                    noteId: id,
                };
            }
            BusEvent::Expression {
                offset,
                id,
                kind,
                value,
            } => {
                event.sampleOffset = offset;
                event.r#type = EXPRESSION;
                event.__field0.noteExpressionValue = NoteExpressionValueEvent {
                    typeId: kind,
                    noteId: id,
                    value,
                };
            }
            BusEvent::SysEx { offset, bytes } => {
                event.sampleOffset = offset;
                event.r#type = DATA;
                event.__field0.data = DataEvent {
                    size: u32::try_from(bytes.len()).unwrap_or(u32::MAX),
                    r#type: SYSEX,
                    bytes: bytes.as_ptr(),
                };
            }
            BusEvent::Controller {
                offset,
                channel,
                controller,
                value,
                value2,
            } => {
                event.sampleOffset = offset;
                event.r#type = LEGACY_CC_OUT;
                event.__field0.midiCCOut = LegacyMIDICCOutEvent {
                    controlNumber: controller,
                    channel: i8::try_from(channel).unwrap_or(0),
                    value,
                    value2,
                };
            }
        }
        event
    }
}

/// The most events one block's list holds without allocating.
const EVENTS: usize = 64;

/// A block's events on the event bus.
struct Events {
    list: RefCell<Vec<Event>>,
}

impl Events {
    fn new() -> Self {
        Events {
            list: RefCell::new(Vec::with_capacity(EVENTS)),
        }
    }

    fn clear(&self) {
        self.list.borrow_mut().clear();
    }

    fn fill(&self, events: &[BusEvent<'_>]) {
        let mut list = self.list.borrow_mut();
        list.clear();
        list.extend(events.iter().map(|event| event.to_vst()));
    }
}

impl Class for Events {
    type Interfaces = (IEventList,);
}

impl IEventListTrait for Events {
    unsafe fn getEventCount(&self) -> int32 {
        int32::try_from(self.list.borrow().len()).unwrap_or(int32::MAX)
    }

    unsafe fn getEvent(&self, index: int32, event: *mut Event) -> tresult {
        let list = self.list.borrow();
        let found = usize::try_from(index)
            .ok()
            .and_then(|index| list.get(index));
        // SAFETY: the exporter passes room for one event, or null.
        match (found, unsafe { event.as_mut() }) {
            (Some(found), Some(event)) => {
                *event = *found;
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn addEvent(&self, event: *mut Event) -> tresult {
        // SAFETY: the plugin passes one event to copy, or null.
        let Some(event) = (unsafe { event.as_ref() }) else {
            return kInvalidArgument;
        };
        self.list.borrow_mut().push(*event);
        kResultOk
    }
}

/// A memory stream.
#[derive(Default)]
pub(super) struct Stream {
    pub(super) bytes: RefCell<Vec<u8>>,
    pub(super) position: Cell<usize>,
}

impl Class for Stream {
    type Interfaces = (IBStream,);
}

impl IBStreamTrait for Stream {
    unsafe fn read(&self, buffer: *mut c_void, count: int32, read: *mut int32) -> tresult {
        let bytes = self.bytes.borrow();
        let start = self.position.get().min(bytes.len());
        let length = usize::try_from(count).unwrap_or(0).min(bytes.len() - start);
        if length > 0 && buffer.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: the caller passes room for `count` bytes; `length` is at
        // most that.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes[start..].as_ptr(), buffer.cast::<u8>(), length);
        }
        self.position.set(start + length);
        if !read.is_null() {
            // SAFETY: the caller passes room for the count.
            unsafe { *read = length as int32 };
        }
        kResultOk
    }

    unsafe fn write(&self, buffer: *mut c_void, count: int32, written: *mut int32) -> tresult {
        let length = usize::try_from(count).unwrap_or(0);
        if length > 0 && buffer.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: the caller passes `count` bytes.
        let source =
            unsafe { std::slice::from_raw_parts(buffer.cast::<u8>().cast_const(), length) };
        let mut bytes = self.bytes.borrow_mut();
        let start = self.position.get().min(bytes.len());
        bytes.truncate(start);
        bytes.extend_from_slice(source);
        self.position.set(start + length);
        if !written.is_null() {
            // SAFETY: the caller passes room for the count.
            unsafe { *written = length as int32 };
        }
        kResultOk
    }

    unsafe fn seek(&self, position: int64, mode: int32, result: *mut int64) -> tresult {
        let length = self.bytes.borrow().len() as int64;
        let base = match mode {
            0 => 0,
            1 => self.position.get() as int64,
            2 => length,
            _ => return kInvalidArgument,
        };
        let target = (base + position).clamp(0, length);
        self.position.set(target as usize);
        if !result.is_null() {
            // SAFETY: the caller passes room for the position.
            unsafe { *result = target };
        }
        kResultOk
    }

    unsafe fn tell(&self, position: *mut int64) -> tresult {
        if position.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: as above.
        unsafe { *position = self.position.get() as int64 };
        kResultOk
    }
}
