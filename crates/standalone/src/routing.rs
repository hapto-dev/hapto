//! Which of the computer's MIDI devices the app opens: a filter for each
//! direction, which the author's code sets before the app runs and may
//! change while it runs, from any thread.

#[cfg(test)]
mod tests;

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use hapto_midi_devices::{Device, Timing};

/// A filter: whether the app opens a device.
type Filter = Arc<dyn Fn(&Device) -> bool + Send + Sync>;

/// The app's MIDI devices, as the author's code reaches them: which ones
/// the app opens, when its outputs send, and which are listed and open
/// now. Made by [`App::midi`](crate::App::midi), and cheap to clone and
/// send to another thread, where a plugin's page or a thread of the
/// author's own changes them while the app runs.
///
/// A filter is asked of each device the system lists, and of each that
/// appears later. A change applies at the app's next tick, on its main
/// thread: the devices that now pass open, and the ones open that no longer
/// pass close. Every input opens by default, and no output, and only for a
/// plugin that takes MIDI, or sends it. The outputs send each block's MIDI
/// as soon as it has played, at the least latency, unless
/// [`set_timing`](Self::set_timing) has them keep each message's frame.
///
/// # Examples
///
/// ```
/// use hapto_standalone::MidiDevices;
///
/// // In a thread of the author's own, or as a page asks: from now on, the
/// // plugin's MIDI goes to a virtual port and the system's synth.
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
    /// Counts the changes, so the app applies each once.
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
    /// Every input, and no output.
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

    /// From now on, opens the inputs `filter` passes: a plugin that takes
    /// MIDI hears them.
    pub fn set_inputs(&self, filter: impl Fn(&Device) -> bool + Send + Sync + 'static) {
        let mut routing = self.lock();
        routing.inputs = Arc::new(filter);
        routing.changes = routing.changes.wrapping_add(1);
    }

    /// From now on, opens the outputs `filter` passes: a plugin that sends
    /// MIDI sends to them.
    pub fn set_outputs(&self, filter: impl Fn(&Device) -> bool + Send + Sync + 'static) {
        let mut routing = self.lock();
        routing.outputs = Arc::new(filter);
        routing.changes = routing.changes.wrapping_add(1);
    }

    /// From now on, has the outputs send as `timing` says: each block's
    /// MIDI as soon as it has played, the default, or each message at its
    /// frame's time, for an arpeggiator, a sequencer or a clock.
    pub fn set_timing(&self, timing: Timing) {
        let mut routing = self.lock();
        routing.timing = timing;
        routing.changes = routing.changes.wrapping_add(1);
    }

    /// When the outputs send.
    pub fn timing(&self) -> Timing {
        self.lock().timing
    }

    /// The system's MIDI devices, inputs and outputs, as of the app's last
    /// tick.
    pub fn listed(&self) -> Vec<Device> {
        self.lock().listed.clone()
    }

    /// The devices open now, inputs and outputs, as of the app's last
    /// tick.
    pub fn open(&self) -> Vec<Device> {
        self.lock().open.clone()
    }

    /// The filters, and how many changes they are past: the app's main
    /// thread calls them with no lock held, so a filter may reach this.
    /// The timing comes with [`timing`](Self::timing).
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

    /// Says what the app's tick found: the system's devices, and the ones
    /// open.
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

/// What a filter's change asks of one direction: of `listed`, each with
/// whether the filter passes it, the ones to open, which are not `open`,
/// and of `open`, the ones to close, which it no longer passes or the
/// system no longer lists.
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
