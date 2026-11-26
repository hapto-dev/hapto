//! The component: one object that is both the audio processor and the edit
//! controller, a shell over the plugin's [`Engine`] that adds the SDK's
//! shape.
//!
//! Threads, as the SDK states them: `process` runs on the audio thread,
//! `setProcessing` on the audio or the UI thread, everything else on the UI
//! thread. The SDK does not say where a host creates the component, so the
//! plugin is built at `initialize`, which starts the component's life; a
//! call made before it answers `kNotInitialized`. The thread that calls
//! `initialize` becomes the main thread, and the threading rule on
//! [`Engine`] does the rest.

use std::ffi::{c_char, c_void};
use std::ptr::NonNull;
use std::sync::OnceLock;

use ::vst3::Steinberg::Vst::{
    BusDirection, BusDirections_, BusInfo, BusInfo_::BusFlags_, BusType, BusTypes_,
    IAudioProcessor, IAudioProcessorTrait, IComponent, IComponentHandler, IComponentHandler2,
    IComponentHandler2Trait, IComponentHandlerTrait, IComponentTrait, IEditController,
    IMidiMapping, INoteExpressionController, IParamValueQueueTrait, IParameterChanges,
    IParameterChangesTrait, IProcessContextRequirements, IProcessContextRequirementsTrait, IoMode,
    MediaType, MediaTypes_, ParamID, ProcessData, ProcessSetup as VstSetup, RestartFlags_,
    RoutingInfo, SpeakerArrangement, SymbolicSampleSizes_, kInfiniteTail, kNoTail,
};
use ::vst3::Steinberg::{
    FIDString, FUnknown, IBStream, IPluginBaseTrait, TBool, TUID, int32, kInternalError,
    kInvalidArgument, kNotImplemented, kNotInitialized, kResultFalse, kResultOk, kResultTrue,
    tresult, uint32,
};
use ::vst3::{Class, ComPtr, ComRef, ComWrapper};
use hapto_params::{HostEvent, ParamDecl, ParamId, ParamSet, audio_thread};

mod buses;
mod controller;
mod midi;
mod midi_out;

use super::view;

use super::stream::{read_all, write_all};
use super::text::copy_utf16;
use crate::boundary::{AudioSlot, guard};
use crate::engine::timer::Source;
use crate::engine::{
    Engine, HostLink, MAX_SKIPPED_QUEUES, MidiIn, Outcome, Points, Release, Restart, Setup,
    TAIL_INFINITE, TAIL_NONE,
};
use crate::layout::Width;
use crate::{MAX_STATE_BYTES, Plugin, first_width, widths};
use buses::{arrangement, bus_channels, channels_of, has_input, set_silence_flags, silence};

// The engine's tail answers are the SDK's.
const _: () = assert!(kNoTail == TAIL_NONE && kInfiniteTail == TAIL_INFINITE);

/// The host's component handler, as the engine reaches it.
pub(crate) type Handler = ComPtr<IComponentHandler>;

impl HostLink for Handler {
    /// A host may have read the latency before activating, as JUCE's
    /// hosting does, so an activation's is told at the next tick.
    const ACTIVATION_TELLS_LATENCY: bool = false;
    /// `process` brings the host's parameter changes as queues.
    const QUEUES: bool = true;

    fn tell(&self, set: &ParamSet, event: HostEvent) {
        // SAFETY: `self` holds a reference, so the host's handler is alive.
        // The engine calls this only on the main thread (inside a tick,
        // `terminate` or `setComponentHandler`), where the SDK lets a plugin
        // call `IComponentHandler`. The engine holds no borrow the host could
        // reach back into.
        unsafe {
            match event {
                HostEvent::Begin(id) => self.beginEdit(set.host_id(id)),
                HostEvent::Value { id, normalized } => {
                    self.performEdit(set.host_id(id), normalized)
                }
                HostEvent::End(id) => self.endEdit(set.host_id(id)),
            };
        }
    }

    fn restart(&self, restart: Restart) {
        let mut flags = 0;
        if restart.values {
            flags |= RestartFlags_::kParamValuesChanged;
        }
        if restart.latency {
            flags |= RestartFlags_::kLatencyChanged;
        }
        // SAFETY: as in `tell`: a live handler, called on the main thread
        // inside a tick, with no borrow held.
        unsafe { self.restartComponent(flags) };
    }

    /// `IComponentHandler2::setDirty`, for a host whose handler has it.
    fn state_changed(&self) {
        if let Some(handler) = self.cast::<IComponentHandler2>() {
            // SAFETY: as in `tell`: the handler the cast answered is live
            // while `handler` holds its reference, and this is the main
            // thread, inside a tick, with no borrow held.
            unsafe { handler.setDirty(1) };
        }
    }
}

/// A plugin as one VST3 component: a shell over its engine.
pub(crate) struct Component<P: Plugin> {
    /// Built at `initialize`.
    engine: OnceLock<Owned<P>>,
    /// Whether `initialize` starts the engine's timer.
    timer: bool,
    /// The address of the component's own `IEditController`, set as it is
    /// made ([`wrap`](Self::wrap)), with no reference of its own: what a
    /// view takes a reference to the component through.
    this: OnceLock<usize>,
    /// The notes that the host gave ids, kept across blocks for their note
    /// expressions. The audio thread holds them in a block, and an activation
    /// clears them.
    notes: AudioSlot<midi::Notes>,
    /// What sending the plugin's MIDI keeps across blocks, the notes it sent
    /// under a host's id and its SysEx room. The audio thread holds it in a
    /// block, and an activation clears it.
    midi_out: AudioSlot<midi_out::Out>,
}

/// The engine's box, freed in the component's `Drop` when the engine says
/// it may be, leaked otherwise. A timer's tick holds the same address.
struct Owned<P: Plugin>(NonNull<Engine<P, Handler>>);

// SAFETY: the pointer stands in for a `Box<Engine>`, which is `Send` and
// `Sync` (the engine checks it of itself).
unsafe impl<P: Plugin> Send for Owned<P> {}
// SAFETY: as above.
unsafe impl<P: Plugin> Sync for Owned<P> {}

impl<P: Plugin> Class for Component<P> {
    type Interfaces = (
        IComponent,
        IAudioProcessor,
        IEditController,
        IMidiMapping,
        INoteExpressionController,
        IProcessContextRequirements,
    );
}

impl<P: Plugin> Component<P> {
    /// Makes a component whose plugin `initialize` builds, as a COM object.
    /// With `timer`, `initialize` also starts the plugin's tick.
    ///
    /// A plugin that declares a bus the exporter cannot arrange fails to
    /// build here, as in [`export_vst3!`](crate::export_vst3).
    pub(crate) fn wrap(timer: bool) -> ComWrapper<Self> {
        const { super::check_layouts(widths::<P>()) };
        let component = ComWrapper::new(Component {
            engine: OnceLock::new(),
            timer,
            this: OnceLock::new(),
            notes: AudioSlot::new(midi::Notes::new()),
            midi_out: AudioSlot::new(midi_out::Out::new(P::INFO.midi_output)),
        });
        if let Some(this) = component.as_com_ref::<IEditController>() {
            let _ = component.this.set(this.as_ptr() as usize);
        }
        component
    }

    /// A reference to this component, as its own `IEditController`.
    fn this(&self) -> Option<ComPtr<IEditController>> {
        let this = *self.this.get()?;
        // SAFETY: the address is this component's own `IEditController`,
        // set as `wrap` made it, and the component is alive while `&self`
        // is. `to_com_ptr` adds the reference the result holds.
        unsafe { ComRef::from_raw(this as *mut IEditController) }.map(|this| this.to_com_ptr())
    }

    /// The engine, once `initialize` built it.
    fn engine(&self) -> Option<&Engine<P, Handler>> {
        // SAFETY: the box came from `Box::leak` in `initialize`, and only
        // `Drop` frees it, so it is live while `&self` is, and only ever
        // shared.
        self.engine.get().map(|owned| unsafe { owned.0.as_ref() })
    }

    /// Whether the engine's processor came with learn's audio side; `None`
    /// before `initialize` or with no processor.
    #[cfg(test)]
    pub(crate) fn holds_learn_audio(&self) -> Option<bool> {
        self.engine()?.holds_learn_audio()
    }

    /// The main bus layout: the engine's, or the first declared before
    /// `initialize`.
    fn layout(&self) -> Width {
        self.engine()
            .map_or(const { first_width::<P>() }, Engine::layout)
    }

    /// Runs an entry point's body, answering `refused` if it panics: inside
    /// the engine's guard once `initialize` built it, so the instance counts
    /// the fault, and inside the process's before.
    fn guard<R>(&self, refused: R, body: impl FnOnce() -> R) -> R {
        match self.engine() {
            Some(engine) => engine.guard(refused, body),
            None => guard(refused, body),
        }
    }

    /// How many faults the instance has had, on every thread; none before
    /// `initialize`.
    #[cfg(any(test, feature = "test-host"))]
    pub(crate) fn faults(&self) -> u64 {
        self.engine().map_or(0, Engine::faults)
    }

    /// The engine's running timer's handle.
    #[cfg(test)]
    pub(crate) fn timer_handle(&self) -> Option<crate::engine::timer::Handle> {
        self.engine().and_then(Engine::timer_handle)
    }

    /// Starts the engine's timer from `source`, as a shell does when a host
    /// gives it a run loop.
    #[cfg(test)]
    pub(crate) fn start_timer(&self, source: Source) {
        self.engine().expect("initialized").start_timer(source);
    }

    /// Every automation point the last block carried, and how many it had
    /// no room for.
    #[cfg(test)]
    pub(crate) fn carried(&self) -> (Vec<crate::engine::Point>, u64) {
        self.engine().expect("initialized").carried()
    }

    /// Returns what `read` makes of the last block's MIDI, decoded as its
    /// processor read it, or `None` before `initialize` or while inactive.
    #[cfg(test)]
    pub(crate) fn heard<T>(&self, read: impl FnOnce(hapto_midi::Midi<'_>) -> T) -> Option<T> {
        self.engine()?.heard(read)
    }

    /// Returns how many of the plugin's messages VST3 had no event for, or
    /// the host refused, since the component was made.
    #[cfg(test)]
    pub(crate) fn lost_out(&self) -> u64 {
        self.midi_out.hold().lost()
    }

    /// Runs one main-thread tick, as `Engine::idle` does. For the test host.
    #[cfg(any(test, feature = "test-host"))]
    pub(crate) fn tick(&self) {
        if let Some(engine) = self.engine() {
            engine.idle();
        }
    }

    /// The tick exactly as the timer holds it; for tests that release the
    /// component inside one.
    #[cfg(test)]
    pub(crate) fn ticker(&self) -> crate::engine::timer::Tick {
        self.engine().expect("initialized").ticker()
    }

    /// Processes one block's parameter changes, then its MIDI, then its
    /// audio.
    ///
    /// # Safety
    ///
    /// `data` is null, or the host's `ProcessData` for this `process` call,
    /// valid for reads and writes until it returns. Its buses, channel
    /// buffers, parameter changes and events are valid as the VST3 SDK
    /// promises for that call. The host touches none of them meanwhile.
    unsafe fn process_block(&self, data: *mut ProcessData) -> tresult {
        // SAFETY: the caller's contract, and `data` is used only inside this
        // call.
        let Some(data) = (unsafe { data.as_mut() }) else {
            return kInvalidArgument;
        };
        let Some(engine) = self.engine() else {
            return kNotInitialized;
        };
        let changes = data.inputParameterChanges;
        let writes = |points: &mut Points<'_>| {
            // SAFETY: the SDK makes `inputParameterChanges` null or the
            // host's `IParameterChanges`, alive with every queue it hands
            // out until `process` returns. `ComRef` borrows it without a
            // reference for no longer than that.
            if let Some(changes) = unsafe { ComRef::from_raw(changes) } {
                // SAFETY: as above.
                unsafe { read_changes(engine.set(), points, changes) };
            }
        };
        if data.symbolicSampleSize != SymbolicSampleSizes_::kSample32 as int32 {
            // The host's writes still land; the block is refused unplayed.
            // SAFETY: no channel is passed, so none needs to be valid.
            unsafe { engine.process(0, &[], &[], writes, |_| {}, |_| {}) };
            return kResultFalse;
        }
        let events = data.inputEvents;
        // As many queues as the automation's walk reads, plus the grid.
        let most = engine
            .set()
            .len()
            .saturating_add(midi::GRID as usize)
            .saturating_add(MAX_SKIPPED_QUEUES);
        let mut notes = self.notes.try_hold();
        let midi = |into: &mut MidiIn<'_>| {
            if !P::INFO.midi_input {
                return;
            }
            // SAFETY: the SDK makes `inputEvents` null or the host's
            // `IEventList`, alive with each SysEx message's bytes until
            // `process` returns. `ComRef` borrows it for no longer than that.
            let events = unsafe { ComRef::from_raw(events) };
            // SAFETY: as above.
            let keep = events.map_or(0, |events| unsafe { midi::event_count(events) });
            // Controllers go first, so at one frame they precede the notes,
            // as in JUCE's wrapper, and they leave the notes their room, so
            // dense controllers drop no note-off.
            // SAFETY: as for the automation above.
            if let Some(changes) = unsafe { ComRef::from_raw(changes) } {
                // SAFETY: as above.
                unsafe { midi::read_controllers(changes, most, keep, into) };
            }
            if let Some(events) = events {
                // SAFETY: as above.
                unsafe { midi::read_events(events, notes.as_deref_mut(), into) };
            }
        };
        let frames = usize::try_from(data.numSamples).unwrap_or(0);
        let output_events = data.outputEvents;
        let mut sending = self.midi_out.try_hold();
        let sent = |packets: &[crate::Event]| {
            if !P::INFO.midi_output {
                return;
            }
            // SAFETY: the SDK makes `outputEvents` null or the host's
            // `IEventList`, alive until `process` returns. `ComRef` borrows
            // it for no longer than that.
            let list = unsafe { ComRef::from_raw(output_events) };
            if let (Some(list), Some(out)) = (list, sending.as_deref_mut()) {
                // SAFETY: as above.
                unsafe { midi_out::write_events(list, packets, out) };
            }
        };
        // SAFETY: the caller's contract: `inputs` and `outputs` are null or
        // hold `numInputs` and `numOutputs` buses for this call, each
        // channel valid for `numSamples` floats. The setup is 32-bit
        // (checked above), as `bus_channels` requires.
        let (inputs, outputs) = unsafe {
            (
                bus_channels(data.inputs, data.numInputs),
                bus_channels(data.outputs, data.numOutputs),
            )
        };
        // SAFETY: as above: each channel holds `frames` floats for this
        // call, and the host touches none of them meanwhile. The engine also
        // asks that any two channels are one buffer or share no float, which
        // the VST3 SDK does not promise but hosts do: a buffer per channel,
        // or an input handed again as its output in place. A host that broke
        // it would get wrong audio, not an unsound access: `Channels` gives
        // no two channels a float in common.
        match unsafe { engine.process(frames, inputs, outputs, writes, midi, sent) } {
            Outcome::Empty => {}
            // SAFETY: as above, as `silence` requires.
            Outcome::Silent => unsafe { silence(data) },
            // SAFETY: the host's output buses are valid for the call.
            Outcome::Played(_) => unsafe { set_silence_flags(data, false) },
        }
        kResultOk
    }

    fn param(&self, host_id: ParamID) -> Option<(ParamId, &ParamDecl)> {
        let set = self.engine()?.set();
        let id = set.by_host_id(host_id)?;
        Some((id, set.decl(id)))
    }
}

/// Hands the engine every queue whose id names a parameter, as the host's
/// automation for the block.
///
/// Each point keeps its sample offset, and the queues go in the host's order,
/// until [`Points::walking`] says the walk is over.
///
/// # Safety
///
/// `changes` and every queue it hands out are alive until this call
/// returns, as the SDK promises for the changes `process` receives.
unsafe fn read_changes(
    set: &ParamSet,
    points: &mut Points<'_>,
    changes: ComRef<'_, IParameterChanges>,
) {
    // SAFETY: the caller's contract, for each call below.
    let claimed = usize::try_from(unsafe { changes.getParameterCount() }).unwrap_or(0);
    for index in 0..claimed {
        if !points.walking() {
            break;
        }
        let index = int32::try_from(index).unwrap_or(int32::MAX);
        // SAFETY: as above.
        let Some(queue) = (unsafe { ComRef::from_raw(changes.getParameterData(index)) }) else {
            points.skip();
            continue;
        };
        // SAFETY: as above.
        let Some(id) = set.by_host_id(unsafe { queue.getParameterId() }) else {
            points.skip();
            continue;
        };
        // SAFETY: as above.
        let count = usize::try_from(unsafe { queue.getPointCount() }).unwrap_or(0);
        points.queue(id, count, |point| {
            let point = int32::try_from(point).ok()?;
            let (mut offset, mut value) = (0, 0.0);
            // SAFETY: as above; `offset` and `value` are room for the point.
            let read = unsafe { queue.getPoint(point, &mut offset, &mut value) };
            (read == kResultOk).then(|| (u32::try_from(offset).unwrap_or(0), value))
        });
    }
}

impl<P: Plugin> Drop for Component<P> {
    fn drop(&mut self) {
        guard((), || {
            let Some(owned) = self.engine.get() else {
                return;
            };
            // SAFETY: as in `engine`: the box is live until freed below.
            if unsafe { owned.0.as_ref() }.release() == Release::Free {
                // SAFETY: the box came from `Box::leak` in `initialize` and
                // is freed once, here. `release` said nothing reaches it any
                // more: this is the main thread, outside any tick, and the
                // timer whose tick held its address is dropped.
                drop(unsafe { Box::from_raw(owned.0.as_ptr()) });
            }
        });
    }
}

impl<P: Plugin> std::fmt::Debug for Component<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Component")
            .field("plugin", &P::INFO.name)
            .finish_non_exhaustive()
    }
}

impl<P: Plugin> IPluginBaseTrait for Component<P> {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        self.guard(kInternalError, || {
            let mut built = false;
            let owned = self.engine.get_or_init(|| {
                built = true;
                Owned(NonNull::from(Box::leak(Engine::new(None))))
            });
            // SAFETY: as in `engine`: the box is live while `&self` is.
            let engine = unsafe { owned.0.as_ref() };
            // A second `initialize` on another thread is refused, and
            // counted.
            if engine.main().is_none() {
                return kResultFalse;
            }
            if built && self.timer {
                engine.start_timer(Source::Thread);
            }
            kResultOk
        })
    }

    unsafe fn terminate(&self) -> tresult {
        self.guard(kInternalError, || {
            let Some(engine) = self.engine() else {
                return kNotInitialized;
            };
            let Some(main) = engine.main() else {
                return kResultFalse;
            };
            engine.connect(&main, None);
            kResultOk
        })
    }
}

impl<P: Plugin> IComponentTrait for Component<P> {
    unsafe fn getControllerClassId(&self, _class_id: *mut TUID) -> tresult {
        // The component is its own controller: a host asks it for
        // `IEditController` instead.
        kNotImplemented
    }

    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kNotImplemented
    }

    unsafe fn getBusCount(&self, media: MediaType, direction: BusDirection) -> int32 {
        let input = direction == BusDirections_::kInput as BusDirection;
        let count = if media == MediaTypes_::kAudio as MediaType {
            !input || has_input::<P>()
        } else if media == MediaTypes_::kEvent as MediaType {
            if input {
                P::INFO.midi_input
            } else {
                P::INFO.midi_output
            }
        } else {
            false
        };
        int32::from(count)
    }

    unsafe fn getBusInfo(
        &self,
        media: MediaType,
        direction: BusDirection,
        index: int32,
        bus: *mut BusInfo,
    ) -> tresult {
        self.guard(kInternalError, || {
            // SAFETY: the SDK's contract: `bus` is null or valid for writing
            // one `BusInfo` until this call returns.
            let Some(bus) = (unsafe { bus.as_mut() }) else {
                return kInvalidArgument;
            };
            // SAFETY: `getBusCount` is this object's own method and reads no
            // pointer, so it has no precondition.
            if index != 0 || unsafe { self.getBusCount(media, direction) } == 0 {
                return kInvalidArgument;
            }
            let input = direction == BusDirections_::kInput as BusDirection;
            let (channels, name) = if media == MediaTypes_::kAudio as MediaType {
                let layout = self.layout();
                if input {
                    (layout.inputs, "Input")
                } else {
                    (layout.outputs, "Output")
                }
            } else if input {
                (16, "MIDI In")
            } else {
                (16, "MIDI Out")
            };
            bus.mediaType = media;
            bus.direction = direction;
            bus.channelCount = channels as int32;
            copy_utf16(name, &mut bus.name);
            bus.busType = BusTypes_::kMain as BusType;
            bus.flags = BusFlags_::kDefaultActive as uint32;
            kResultOk
        })
    }

    unsafe fn getRoutingInfo(&self, _in: *mut RoutingInfo, _out: *mut RoutingInfo) -> tresult {
        kNotImplemented
    }

    unsafe fn activateBus(
        &self,
        media: MediaType,
        direction: BusDirection,
        index: int32,
        _state: TBool,
    ) -> tresult {
        // SAFETY: `getBusCount` is this object's own method and reads no
        // pointer, so it has no precondition.
        if index == 0 && unsafe { self.getBusCount(media, direction) } == 1 {
            kResultOk
        } else {
            kInvalidArgument
        }
    }

    unsafe fn setActive(&self, state: TBool) -> tresult {
        self.guard(kInternalError, || {
            let Some(engine) = self.engine() else {
                return kNotInitialized;
            };
            let Some(main) = engine.main() else {
                return kResultFalse;
            };
            let done = if state != 0 {
                // The kept notes belong to the last activation.
                self.notes.hold().clear();
                self.midi_out.hold().clear();
                let setup = main.setup().unwrap_or(Setup::FALLBACK);
                engine.activate(&main, setup, engine.layout())
            } else {
                engine.deactivate(&main)
            };
            if done { kResultOk } else { kResultFalse }
        })
    }

    unsafe fn setState(&self, stream: *mut IBStream) -> tresult {
        self.guard(kInternalError, || {
            let Some(engine) = self.engine() else {
                return kNotInitialized;
            };
            // SAFETY: the SDK's contract: `stream` is null or an `IBStream`
            // the host keeps alive until this call returns, as `read_all`
            // requires. A longer block fails the call unread.
            let Some(bytes) = (unsafe { read_all(stream, MAX_STATE_BYTES) }) else {
                return kResultFalse;
            };
            if engine.load_saved(&bytes).is_ok() {
                kResultOk
            } else {
                kResultFalse
            }
        })
    }

    unsafe fn getState(&self, stream: *mut IBStream) -> tresult {
        self.guard(kInternalError, || {
            let Some(engine) = self.engine() else {
                return kNotInitialized;
            };
            let bytes = engine.save();
            // SAFETY: the SDK's contract: `stream` is null or an `IBStream`
            // the host keeps alive until this call returns, as `write_all`
            // requires.
            if unsafe { write_all(stream, &bytes) } {
                kResultOk
            } else {
                kResultFalse
            }
        })
    }
}

impl<P: Plugin> IAudioProcessorTrait for Component<P> {
    unsafe fn setBusArrangements(
        &self,
        inputs: *mut SpeakerArrangement,
        input_count: int32,
        outputs: *mut SpeakerArrangement,
        output_count: int32,
    ) -> tresult {
        self.guard(kInternalError, || {
            let Some(engine) = self.engine() else {
                return kNotInitialized;
            };
            let Some(main) = engine.main() else {
                return kResultFalse;
            };
            if main.active() {
                return kResultFalse;
            }
            let expected_inputs = int32::from(has_input::<P>());
            if input_count != expected_inputs || output_count != 1 || outputs.is_null() {
                return kResultFalse;
            }
            let inputs = if expected_inputs == 1 {
                // SAFETY: the SDK's contract: `inputs` holds `input_count`
                // arrangements, and `input_count` is 1 and `inputs` non-null
                // here.
                match (!inputs.is_null()).then(|| channels_of(unsafe { *inputs })) {
                    Some(Some(channels)) => channels,
                    _ => return kResultFalse,
                }
            } else {
                0
            };
            // SAFETY: the SDK's contract: `outputs` holds `output_count`
            // arrangements, and `output_count` is 1 and `outputs` non-null
            // here.
            let Some(outputs) = channels_of(unsafe { *outputs }) else {
                return kResultFalse;
            };
            if engine.arrange(&main, Width::new(inputs, outputs)) {
                kResultTrue
            } else {
                kResultFalse
            }
        })
    }

    unsafe fn getBusArrangement(
        &self,
        direction: BusDirection,
        index: int32,
        arrangement_out: *mut SpeakerArrangement,
    ) -> tresult {
        self.guard(kInternalError, || {
            // SAFETY: the SDK's contract: `arrangement_out` is null or valid
            // for writing one arrangement until this call returns.
            let Some(out) = (unsafe { arrangement_out.as_mut() }) else {
                return kInvalidArgument;
            };
            let input = direction == BusDirections_::kInput as BusDirection;
            if index != 0 || (input && !has_input::<P>()) {
                return kInvalidArgument;
            }
            let layout = self.layout();
            match arrangement(if input { layout.inputs } else { layout.outputs }) {
                Some(arrangement) => {
                    *out = arrangement;
                    kResultOk
                }
                None => kInvalidArgument,
            }
        })
    }

    unsafe fn canProcessSampleSize(&self, size: int32) -> tresult {
        if size == SymbolicSampleSizes_::kSample32 as int32 {
            kResultTrue
        } else {
            kResultFalse
        }
    }

    unsafe fn getLatencySamples(&self) -> uint32 {
        self.engine().map_or(0, Engine::latency)
    }

    unsafe fn setupProcessing(&self, setup: *mut VstSetup) -> tresult {
        self.guard(kInternalError, || {
            // SAFETY: the SDK's contract: `setup` is null or valid for
            // reading one `ProcessSetup` until this call returns.
            let Some(setup) = (unsafe { setup.as_ref() }) else {
                return kInvalidArgument;
            };
            let Some(engine) = self.engine() else {
                return kNotInitialized;
            };
            let Some(main) = engine.main() else {
                return kResultFalse;
            };
            if setup.symbolicSampleSize != SymbolicSampleSizes_::kSample32 as int32 {
                return kResultFalse;
            }
            // A negative block size says nothing, as 0 does.
            let max_frames = usize::try_from(setup.maxSamplesPerBlock).unwrap_or(0);
            let Some(setup) = Setup::new(setup.sampleRate, max_frames) else {
                return kInvalidArgument;
            };
            if engine.set_setup(&main, setup) {
                kResultOk
            } else {
                kResultFalse
            }
        })
    }

    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    unsafe fn setProcessing(&self, state: TBool) -> tresult {
        // Either thread, the audio thread included, so a debug build checks
        // it takes no lock. `setParamNormalized` stays unmarked: the SDK
        // puts it on the UI thread.
        let _audio = audio_thread::enter();
        match self.engine() {
            Some(engine) => {
                engine.set_processing(state != 0);
                kResultOk
            }
            None => kNotInitialized,
        }
    }

    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        // SAFETY: the SDK's contract for `process`: `data` is null or the
        // host's process data for this call. Its buses and changes are valid
        // until it returns and untouched by the host meanwhile, which is
        // `process_block`'s contract.
        self.guard(kInternalError, || unsafe { self.process_block(data) })
    }

    unsafe fn getTailSamples(&self) -> uint32 {
        self.engine().map_or(kNoTail, Engine::tail)
    }
}

impl<P: Plugin> IProcessContextRequirementsTrait for Component<P> {
    unsafe fn getProcessContextRequirements(&self) -> uint32 {
        0
    }
}

/// The class id as the SDK's type.
pub(crate) fn class_id<P: Plugin>() -> TUID {
    P::INFO.vst3_class_id.map(|byte| byte as c_char)
}

/// Creates a component and hands the host the interface it asked for.
///
/// # Safety
///
/// `interface` is null or points to a 16-byte interface id, readable during
/// the call, and `object` is null or valid for writing one pointer.
pub(crate) unsafe fn create<P: Plugin>(interface: FIDString, object: *mut *mut c_void) -> tresult {
    if object.is_null() || interface.is_null() {
        return kInvalidArgument;
    }
    // SAFETY: the caller's contract, with `object` non-null here.
    unsafe { *object = std::ptr::null_mut() };
    guard(kInternalError, || {
        let component = Component::<P>::wrap(true);
        let Some(unknown) = component.to_com_ptr::<FUnknown>() else {
            return kInternalError;
        };
        let unknown = unknown.as_ptr();
        // SAFETY: `unknown` is the live object just made, held by the
        // `ComPtr` above; `interface` and `object` meet `queryInterface`'s
        // contract by this function's. `queryInterface` adds the reference
        // the host gets, and `component` drops its own when it goes.
        unsafe { ((*(*unknown).vtbl).queryInterface)(unknown, interface.cast::<TUID>(), object) }
    })
}
