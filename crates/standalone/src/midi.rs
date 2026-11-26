//! The computer's MIDI devices, played into and out of the plugin.
//!
//! The app opens the inputs and outputs that the author's filters pass. The
//! inputs are merged on group 1, as a host merges them, and each output gets
//! every message that the plugin sends. The open devices follow the system's
//! list and the filters as they change. A line tells the author each device
//! that opens, leaves, closes or fails to open.

use std::time::{Duration, Instant};

use hapto_midi_devices::{
    Device, DeviceId, Devices, Direction, Inputs, InputsReader, OpenDevices, OpenError, Outputs,
    OutputsWriter,
};

use crate::routing::{MidiDevices, plan};

/// How many packets each input holds between two blocks, and each output
/// holds before sending, beyond which they are dropped and counted.
const ROOM: usize = 1024;

/// How long a device that left is kept from opening again.
///
/// The system's list might show it until the list's last read, four seconds
/// after a change.
const LEFT_WAIT: Duration = Duration::from_secs(5);

/// The app's MIDI devices, on the main thread.
#[derive(Debug)]
pub(crate) struct Midi {
    devices: Devices,
    /// The inputs that a stream reads while one plays, if the plugin takes
    /// MIDI.
    inputs: Option<Inputs>,
    /// The outputs that a stream writes to while one plays, if the plugin
    /// sends MIDI.
    outputs: Option<Outputs>,
    /// What the last line for each device said.
    said: Said,
    /// The devices that left and when, kept from opening while the list might
    /// still show them.
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
    /// `takes` MIDI or `sends` it, as `routing` selects.
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

    /// Opens the devices that the filters pass again for a new stream, and
    /// returns the stream's reader and writer.
    ///
    /// An older stream's devices close first, because WinMM lets one opener
    /// at a time hold a device.
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

    /// Closes every device when the audio stops.
    ///
    /// The ports go back to other apps, and each output's reset ends its
    /// notes. No line is printed, and the next stream opens the devices again
    /// without one.
    pub(crate) fn stop(&mut self) {
        drop(self.inputs.take());
        drop(self.outputs.take());
        self.report();
    }

    /// Opens the devices that were added or now pass, and closes the ones
    /// that were removed, left or no longer pass.
    ///
    /// Call it once per tick.
    pub(crate) fn tick(&mut self, now: Instant) {
        let changes = self.devices.tick(now);
        let (count, takes, sends) = self.routing.filters();
        let reapply = count != self.applied;
        let mut moved = !changes.is_empty() || reapply;
        // A device that was added again, or whose wait is over, can open.
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
            follow(self.name, &listed, inputs, &mut self.said);
        }
        if let Some(outputs) = &mut self.outputs {
            outputs.set_timing(self.routing.timing());
            let listed: Vec<(&Device, bool)> = self
                .devices
                .outputs()
                .map(|device| (device, passes(device, &*sends)))
                .collect();
            follow(self.name, &listed, outputs, &mut self.said);
        }
        self.report();
    }

    /// Reports to the routing which devices are listed and open now.
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

/// Opens the `listed` devices that pass and aren't open, and closes the open
/// ones that don't pass or aren't listed, with a line for each.
///
/// Devices match by id, whatever their names.
fn follow(name: &str, listed: &[(&Device, bool)], ends: &mut impl OpenDevices, said: &mut Said) {
    let keyed: Vec<(DeviceId, bool)> = listed
        .iter()
        .map(|&(device, passes)| (device.id().clone(), passes))
        .collect();
    let open: Vec<Device> = ends.devices().cloned().collect();
    let open_ids: Vec<DeviceId> = open.iter().map(|device| device.id().clone()).collect();
    let (to_open, to_close) = plan(&keyed, &open_ids);
    for id in &to_close {
        if let Some(device) = open.iter().find(|device| device.id() == id)
            && ends.close(device.id())
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

/// The last line printed for each device, saying that it opened or why it
/// didn't.
///
/// A stream that starts again, or keeps failing to, prints no new line.
#[derive(Debug)]
struct Said<K = DeviceId>(Vec<(K, Result<(), OpenError>)>);

impl<K> Default for Said<K> {
    fn default() -> Self {
        Said(Vec::new())
    }
}

impl Said {
    /// Prints that `device` opened, or why it didn't, unless the last line
    /// for it said the same.
    fn open(&mut self, name: &str, device: &Device, opened: Result<(), OpenError>) {
        if self.due(device.id(), opened) {
            say(name, device, opened);
        }
    }
}

impl<K: PartialEq + Clone> Said<K> {
    /// Records `opened` as the last line for `id`, and returns whether it
    /// differs from the line before.
    fn due(&mut self, id: &K, opened: Result<(), OpenError>) -> bool {
        let due = !self.0.contains(&(id.clone(), opened));
        self.forget(id);
        self.0.push((id.clone(), opened));
        due
    }

    /// Forgets the last line for `id`, which closed or left with a line of
    /// its own.
    fn forget(&mut self, id: &K) {
        self.0.retain(|(said, _)| said != id);
    }
}

/// Returns the line for `device`, MIDI in from it or out to it.
fn line(device: &Device) -> String {
    match device.direction() {
        Direction::Input => format!("MIDI in from {}", device.name()),
        Direction::Output => format!("MIDI out to {}", device.name()),
    }
}

/// Prints that `device` opened, or why it didn't.
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
