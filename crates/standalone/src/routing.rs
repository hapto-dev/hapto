//! Which of the computer's MIDI devices the app opens.
//!
//! The author's code sets a filter for each direction before the app runs,
//! and can change it from any thread while it runs.

#[cfg(test)]
mod tests;

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use hapto_midi_devices::{Device, Timing};
use hapto_params::audio_thread;

/// A filter, which returns whether the app opens a device.
type Filter = Arc<dyn Fn(&Device) -> bool + Send + Sync>;

/// The app's MIDI devices, as the author's code reaches them.
///
/// It sets which devices the app opens and when its outputs send, and
/// reports which are listed and open now. [`App::midi`](crate::App::midi)
/// makes it, and it's cheap to clone and send to another thread, where a
/// plugin's page or the author's own thread can change them.
///
/// Each call takes a lock that the main thread holds briefly each tick, so
/// no call may run on the audio thread, where a debug build panics.
/// It lists and opens nothing for a plugin that neither takes nor sends
/// MIDI, or under `--host`, where the plugin's format gets no MIDI from the
/// app.
///
/// A filter is called for each device that the system lists, now or later.
/// A change applies at the app's next tick, on its main thread, so the
/// devices that now pass open and the open ones that no longer pass close.
/// By default, every input opens and no output, only for a plugin that takes
/// or sends MIDI. The outputs send each block's MIDI as soon as it has
/// played, for the least latency, unless
/// [`set_timing`](Self::set_timing) has them keep each message's frame.
///
/// # Examples
///
/// ```
/// use hapto_standalone::MidiDevices;
///
/// // From now on, the plugin's MIDI goes to a virtual port and the system's
/// // synth, from the author's own thread or at a page's request.
/// fn send_to_ports(midi: &MidiDevices) {
///     midi.set_outputs(|device| {
///         device.name().starts_with("loopMIDI") || device.name() == "Microsoft GS Wavetable Synth"
///     });
///     for device in midi.open() {
///         println!("{} is open", device.name());
///     }
/// }
/// ```
#[derive(Clone)]
pub struct MidiDevices {
    shared: Arc<Mutex<Routing>>,
}

struct Routing {
    inputs: Filter,
    outputs: Filter,
    timing: Timing,
    /// The number of changes, so the app applies each once.
    changes: u64,
    /// The system's devices, and the ones open, as of the app's last tick.
    listed: Vec<Device>,
    open: Vec<Device>,
}

impl fmt::Debug for MidiDevices {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let routing = self.lock();
        f.debug_struct("MidiDevices")
            .field("listed", &routing.listed)
            .field("open", &routing.open)
            .finish_non_exhaustive()
    }
}

impl MidiDevices {
    /// Creates the routing that opens every input and no output.
    pub(crate) fn new() -> MidiDevices {
        MidiDevices {
            shared: Arc::new(Mutex::new(Routing {
                inputs: Arc::new(|_| true),
                outputs: Arc::new(|_| false),
                timing: Timing::default(),
                changes: 0,
                listed: Vec::new(),
                open: Vec::new(),
            })),
        }
    }

    /// Opens the inputs that `filter` passes from now on, for a plugin that
    /// takes MIDI.
    ///
    /// Thread: any but audio.
    pub fn set_inputs(&self, filter: impl Fn(&Device) -> bool + Send + Sync + 'static) {
        audio_thread::assert_not_here("MidiDevices::set_inputs");
        let mut routing = self.lock();
        routing.inputs = Arc::new(filter);
        routing.changes = routing.changes.wrapping_add(1);
    }

    /// Opens the outputs that `filter` passes from now on, for a plugin that
    /// sends MIDI.
    ///
    /// Thread: any but audio.
    pub fn set_outputs(&self, filter: impl Fn(&Device) -> bool + Send + Sync + 'static) {
        audio_thread::assert_not_here("MidiDevices::set_outputs");
        let mut routing = self.lock();
        routing.outputs = Arc::new(filter);
        routing.changes = routing.changes.wrapping_add(1);
    }

    /// Sets the outputs' timing to `timing` from now on.
    ///
    /// Thread: any but audio.
    pub fn set_timing(&self, timing: Timing) {
        audio_thread::assert_not_here("MidiDevices::set_timing");
        let mut routing = self.lock();
        routing.timing = timing;
        routing.changes = routing.changes.wrapping_add(1);
    }

    /// Returns when the outputs send.
    ///
    /// Thread: any but audio.
    pub fn timing(&self) -> Timing {
        audio_thread::assert_not_here("MidiDevices::timing");
        self.lock().timing
    }

    /// Returns the system's MIDI devices, inputs and outputs, as of the
    /// app's last tick.
    ///
    /// Thread: any but audio.
    pub fn listed(&self) -> Vec<Device> {
        audio_thread::assert_not_here("MidiDevices::listed");
        self.lock().listed.clone()
    }

    /// Returns the open devices, inputs and outputs, as of the app's last
    /// tick.
    ///
    /// Thread: any but audio.
    pub fn open(&self) -> Vec<Device> {
        audio_thread::assert_not_here("MidiDevices::open");
        self.lock().open.clone()
    }

    /// Returns the filters and their change count.
    ///
    /// The main thread calls the filters with no lock held, so a filter can
    /// call this routing. The timing comes from
    /// [`timing`](Self::timing).
    #[cfg_attr(
        all(not(windows), not(test)),
        expect(dead_code, reason = "the app that applies them runs on Windows alone")
    )]
    pub(crate) fn filters(&self) -> (u64, Filter, Filter) {
        let routing = self.lock();
        (
            routing.changes,
            Arc::clone(&routing.inputs),
            Arc::clone(&routing.outputs),
        )
    }

    /// Records the system's devices and the open ones, as the app's tick
    /// found them.
    #[cfg_attr(
        not(windows),
        expect(dead_code, reason = "the app that reports runs on Windows alone")
    )]
    pub(crate) fn report(&self, listed: &[Device], open: Vec<Device>) {
        let mut routing = self.lock();
        routing.listed.clear();
        routing.listed.extend_from_slice(listed);
        routing.open = open;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Routing> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Returns the devices to open and close in one direction after a filter
/// changes.
///
/// `listed` pairs each listed device with whether the filter passes it. A
/// device opens if it passes and isn't in `open`, and an open one closes if
/// it no longer passes or isn't listed.
#[cfg_attr(
    all(not(windows), not(test)),
    expect(dead_code, reason = "the app that applies it runs on Windows alone")
)]
pub(crate) fn plan<K: PartialEq + Clone>(listed: &[(K, bool)], open: &[K]) -> (Vec<K>, Vec<K>) {
    let to_open = listed
        .iter()
        .filter(|(key, passes)| *passes && !open.contains(key))
        .map(|(key, _)| key.clone())
        .collect();
    let to_close = open
        .iter()
        .filter(|key| {
            !listed
                .iter()
                .any(|(listed, passes)| listed == *key && *passes)
        })
        .cloned()
        .collect();
    (to_open, to_close)
}
