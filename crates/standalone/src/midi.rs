//! The computer's MIDI devices, played into and out of the plugin: the
//! inputs and outputs the author's filters pass open, merged on group 1 as
//! a host merges them, followed as devices come and go and as the filters
//! change. A line tells the author each device that opens, leaves, closes
//! or is refused.

use std::time::{Duration, Instant};

use hapto_midi_devices::{
    Device, DeviceId, Devices, Direction, Inputs, InputsReader, OpenError, Outputs, OutputsWriter,
};

use crate::routing::{MidiDevices, plan};

/// The packets each input holds between two blocks, and each output holds
/// waiting to go: past it, they are dropped and counted.
const ROOM: usize = 1024;

/// How long a device that left is kept out of what opens: longer than the
/// system's list may still show it, which the list's last look after a
/// change, at four seconds, settles.
const LEFT_WAIT: Duration = Duration::from_secs(5);

/// The app's MIDI devices, on the main thread.
#[derive(Debug)]
pub(crate) struct Midi {
    devices: Devices,
    /// The inputs a stream reads, while one plays, for a plugin that takes
    /// MIDI.
    inputs: Option<Inputs>,
    /// The outputs a stream writes to, while one plays, for a plugin that
    /// sends MIDI.
    outputs: Option<Outputs>,
    /// What the lines last said of each device.
    said: Said,
    /// The devices that left, and when, kept out of what opens while the
    /// list may still show them.
    departed: Vec<(DeviceId, Instant)>,
    routing: MidiDevices,
    /// The routing's changes applied so far.
    applied: u64,
    takes: bool,
    sends: bool,
    /// The plugin's name, for the lines.
    name: &'static str,
}

impl Midi {
    /// Reads the system's list, opening nothing yet, for a plugin that
    /// `takes` MIDI or `sends` it, as `routing` says.
    pub(crate) fn new(name: &'static str, takes: bool, sends: bool, routing: MidiDevices) -> Midi {
        Midi {
            devices: Devices::new(),
            inputs: None,
            outputs: None,
            said: Said::default(),
            departed: Vec::new(),
            routing,
            applied: 0,
            takes,
            sends,
            name,
        }
    }

    /// Opens the devices the filters pass again for a new stream, and
    /// returns the reader it plays into its blocks and the writer it plays
    /// out of them. An older stream's devices close first: WinMM lets one
    /// opener at a time hold a device.
    pub(crate) fn streams(&mut self) -> (Option<InputsReader>, Option<OutputsWriter>) {
        self.stop();
        let (applied, takes, sends) = self.routing.filters();
        self.applied = applied;
        let departed = &self.departed;
        let kept_out = |device: &Device| departed.iter().any(|(id, _)| id == device.id());
        let reader = self.takes.then(|| {
            let (mut inputs, reader) = hapto_midi_devices::inputs(ROOM);
            for device in self
                .devices
                .inputs()
                .filter(|device| takes(device) && !kept_out(device))
            {
                self.said.open(self.name, device, inputs.open(device));
            }
            self.inputs = Some(inputs);
            reader
        });
        let writer = self.sends.then(|| {
            let (mut outputs, writer) = hapto_midi_devices::outputs(ROOM);
            outputs.set_timing(self.routing.timing());
            for device in self
                .devices
                .outputs()
                .filter(|device| sends(device) && !kept_out(device))
            {
                self.said.open(self.name, device, outputs.open(device));
            }
            self.outputs = Some(outputs);
            writer
        });
        self.report();
        (reader, writer)
    }

    /// Closes every device as the audio stops: their ports go back to other
    /// apps, and each output's reset ends the notes it was told to play.
    /// Nothing is said: the next stream opens them again, unsaid.
    pub(crate) fn stop(&mut self) {
        drop(self.inputs.take());
        drop(self.outputs.take());
        self.report();
    }

    /// Follows the system's list and the routing: opens what came or now
    /// passes, and closes what went, left or no longer passes. Call it once
    /// per tick.
    pub(crate) fn tick(&mut self, now: Instant) {
        let changes = self.devices.tick(now);
        let (count, takes, sends) = self.routing.filters();
        let reapply = count != self.applied;
        let mut moved = !changes.added.is_empty() || !changes.removed.is_empty() || reapply;
        // A device that came again, or whose wait is over, may open again.
        let kept = self.departed.len();
        self.departed.retain(|(id, left)| {
            now.duration_since(*left) < LEFT_WAIT && !changes.added.iter().any(|d| d.id() == id)
        });
        moved |= self.departed.len() != kept;
        let mut left = Vec::new();
        if let Some(inputs) = &mut self.inputs {
            left.extend(inputs.tick());
        }
        if let Some(outputs) = &mut self.outputs {
            left.extend(outputs.tick());
        }
        for device in left {
            eprintln!("{}: {} left", self.name, line(&device));
            self.said.forget(device.id());
            self.departed.push((device.id().clone(), now));
            moved = true;
        }
        if !moved {
            return;
        }
        self.applied = count;
        let departed = &self.departed;
        let passes = |device: &Device, filter: &dyn Fn(&Device) -> bool| {
            filter(device) && !departed.iter().any(|(id, _)| id == device.id())
        };
        if let Some(inputs) = &mut self.inputs {
            let listed: Vec<(&Device, bool)> = self
                .devices
                .inputs()
                .map(|device| (device, passes(device, &*takes)))
                .collect();
            follow(self.name, &listed, Ends::In(inputs), &mut self.said);
        }
        if let Some(outputs) = &mut self.outputs {
            outputs.set_timing(self.routing.timing());
            let listed: Vec<(&Device, bool)> = self
                .devices
                .outputs()
                .map(|device| (device, passes(device, &*sends)))
                .collect();
            follow(self.name, &listed, Ends::Out(outputs), &mut self.said);
        }
        self.report();
    }

    /// Tells the routing what is listed and open now.
    fn report(&self) {
        let open = self
            .inputs
            .iter()
            .flat_map(Inputs::devices)
            .chain(self.outputs.iter().flat_map(Outputs::devices))
            .cloned()
            .collect();
        self.routing.report(self.devices.all(), open);
    }
}

/// One direction's devices, open or to open.
enum Ends<'a> {
    In(&'a mut Inputs),
    Out(&'a mut Outputs),
}

impl Ends<'_> {
    fn open_now(&self) -> Vec<Device> {
        match self {
            Ends::In(inputs) => inputs.devices().cloned().collect(),
            Ends::Out(outputs) => outputs.devices().cloned().collect(),
        }
    }

    fn open(&mut self, device: &Device) -> Result<(), OpenError> {
        match self {
            Ends::In(inputs) => inputs.open(device),
            Ends::Out(outputs) => outputs.open(device),
        }
    }

    fn close(&mut self, device: &Device) -> bool {
        match self {
            Ends::In(inputs) => inputs.close(device.id()),
            Ends::Out(outputs) => outputs.close(device.id()),
        }
    }
}

/// Opens the `listed` devices that pass and are not open, and closes the
/// open ones that do not pass or are not listed, saying so for each. A
/// device is the same one by its id, whatever its name.
fn follow(name: &str, listed: &[(&Device, bool)], mut ends: Ends<'_>, said: &mut Said) {
    let keyed: Vec<(DeviceId, bool)> = listed
        .iter()
        .map(|&(device, passes)| (device.id().clone(), passes))
        .collect();
    let open = ends.open_now();
    let open_ids: Vec<DeviceId> = open.iter().map(|device| device.id().clone()).collect();
    let (to_open, to_close) = plan(&keyed, &open_ids);
    for id in &to_close {
        if let Some(device) = open.iter().find(|device| device.id() == id)
            && ends.close(device)
        {
            eprintln!("{name}: {} closed", line(device));
            said.forget(id);
        }
    }
    for id in &to_open {
        if let Some(&(device, _)) = listed.iter().find(|(device, _)| device.id() == id) {
            said.open(name, device, ends.open(device));
        }
    }
}

/// What the lines last said of each device: that it opened, or why it did
/// not. A stream that starts again, or keeps failing to, says nothing new.
#[derive(Debug)]
struct Said<K = DeviceId>(Vec<(K, Result<(), OpenError>)>);

impl<K> Default for Said<K> {
    fn default() -> Self {
        Said(Vec::new())
    }
}

impl Said {
    /// Says that `device` opened, or why it did not, unless that is what
    /// was said of it last.
    fn open(&mut self, name: &str, device: &Device, opened: Result<(), OpenError>) {
        if self.due(device.id(), opened) {
            say(name, device, opened);
        }
    }
}

impl<K: PartialEq + Clone> Said<K> {
    /// Whether a line is due for `id` that `opened` says, which it is unless
    /// it was said last, and keeps it as said.
    fn due(&mut self, id: &K, opened: Result<(), OpenError>) -> bool {
        let due = !self.0.contains(&(id.clone(), opened));
        self.forget(id);
        self.0.push((id.clone(), opened));
        due
    }

    /// Forgets what was said of `id`, which closed or left with a line of
    /// its own.
    fn forget(&mut self, id: &K) {
        self.0.retain(|(said, _)| said != id);
    }
}

/// The line for `device`: MIDI in from it, or out to it.
fn line(device: &Device) -> String {
    match device.direction() {
        Direction::Input => format!("MIDI in from {}", device.name()),
        Direction::Output => format!("MIDI out to {}", device.name()),
    }
}

/// Says that `device` opened, or why it did not.
fn say(name: &str, device: &Device, opened: Result<(), OpenError>) {
    match opened {
        Ok(()) => eprintln!("{name}: {}", line(device)),
        Err(why) => eprintln!("{name}: {} did not open: {why}", line(device)),
    }
}

#[cfg(test)]
mod tests {
    use hapto_midi_devices::OpenError;

    use super::Said;

    #[test]
    fn a_device_is_said_open_or_refused_once_until_it_changes() {
        let mut said = Said::<&str>::default();
        let keys = "Keys";
        assert!(said.due(&keys, Ok(())), "opened: said");
        assert!(
            !said.due(&keys, Ok(())),
            "open again with the next stream: not"
        );
        assert!(said.due(&keys, Err(OpenError::InUse)), "refused: said");
        assert!(
            !said.due(&keys, Err(OpenError::InUse)),
            "refused again: not"
        );
        said.forget(&keys);
        assert!(
            said.due(&keys, Ok(())),
            "after it left or closed: said again"
        );
    }
}
