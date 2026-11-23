//! The computer's MIDI devices, played into and out of the plugin: the
//! inputs and outputs the author's filters pass open, merged on group 1 as
//! a host merges them, followed as devices come and go and as the filters
//! change. A line tells the author each device that opens, leaves, closes
//! or is refused.

use hapto_midi_devices::{
    Device, Devices, Direction, Inputs, InputsReader, OpenError, Outputs, OutputsWriter,
};

use crate::routing::{MidiDevices, plan};

/// The packets each input holds between two blocks, and each output holds
/// waiting to go: past it, they are dropped and counted.
const ROOM: usize = 1024;

/// The app's MIDI devices, on the main thread.
#[derive(Debug)]
pub(crate) struct Midi {
    devices: Devices,
    /// The inputs a stream reads, while one does, for a plugin that takes
    /// MIDI.
    inputs: Option<Inputs>,
    /// The outputs a stream writes to, while one does, for a plugin that
    /// sends MIDI.
    outputs: Option<Outputs>,
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
        drop(self.inputs.take());
        drop(self.outputs.take());
        let (applied, takes, sends) = self.routing.filters();
        self.applied = applied;
        let reader = self.takes.then(|| {
            let (mut inputs, reader) = hapto_midi_devices::inputs(ROOM);
            for device in self.devices.inputs().filter(|device| takes(device)) {
                say(self.name, device, inputs.open(device));
            }
            self.inputs = Some(inputs);
            reader
        });
        let writer = self.sends.then(|| {
            let (mut outputs, writer) = hapto_midi_devices::outputs(ROOM);
            for device in self.devices.outputs().filter(|device| sends(device)) {
                say(self.name, device, outputs.open(device));
            }
            self.outputs = Some(outputs);
            writer
        });
        self.report();
        (reader, writer)
    }

    /// Follows the system's list and the routing: opens what came or now
    /// passes, and closes what went, left or no longer passes. Call it once
    /// per tick.
    pub(crate) fn tick(&mut self, now: std::time::Instant) {
        let changes = self.devices.tick(now);
        let (count, takes, sends) = self.routing.filters();
        let reapply = count != self.applied;
        let mut moved = !changes.added.is_empty() || !changes.removed.is_empty() || reapply;
        if let Some(inputs) = &mut self.inputs {
            for device in inputs.tick() {
                eprintln!("{}: MIDI in from {} left", self.name, device.name());
                moved = true;
            }
        }
        if let Some(outputs) = &mut self.outputs {
            for device in outputs.tick() {
                eprintln!("{}: MIDI out to {} left", self.name, device.name());
                moved = true;
            }
        }
        if !moved {
            return;
        }
        self.applied = count;
        if let Some(inputs) = &mut self.inputs {
            let listed: Vec<(&Device, bool)> = self
                .devices
                .inputs()
                .map(|device| (device, takes(device)))
                .collect();
            follow(self.name, &listed, Ends::In(inputs));
        }
        if let Some(outputs) = &mut self.outputs {
            let listed: Vec<(&Device, bool)> = self
                .devices
                .outputs()
                .map(|device| (device, sends(device)))
                .collect();
            follow(self.name, &listed, Ends::Out(outputs));
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
/// open ones that do not pass or are not listed, saying so for each.
fn follow(name: &str, listed: &[(&Device, bool)], mut ends: Ends<'_>) {
    let keyed: Vec<(Device, bool)> = listed
        .iter()
        .map(|&(device, passes)| (device.clone(), passes))
        .collect();
    let open = ends.open_now();
    let (to_open, to_close) = plan(&keyed, &open);
    for device in &to_close {
        if ends.close(device) {
            eprintln!("{name}: {} closed", line(device));
        }
    }
    for device in &to_open {
        say(name, device, ends.open(device));
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
