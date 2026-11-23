//! The computer's MIDI inputs, played into a plugin that takes MIDI: every
//! input open, merged on group 1 as a host merges them, followed as devices
//! come and go.
//! A line tells the author each input that opens, leaves or is refused.

use hapto_midi_devices::{Device, Devices, Direction, Inputs, InputsReader};

/// The packets each input holds between two blocks: past it, the input's
/// packets are dropped and counted.
const ROOM: usize = 1024;

/// The app's MIDI inputs, on the main thread.
#[derive(Debug)]
pub(crate) struct Midi {
    devices: Devices,
    /// The inputs a stream reads, while one does.
    inputs: Option<Inputs>,
    /// The plugin's name, for the lines.
    name: &'static str,
}

impl Midi {
    /// Reads the system's list, opening nothing yet.
    pub(crate) fn new(name: &'static str) -> Midi {
        Midi {
            devices: Devices::new(),
            inputs: None,
            name,
        }
    }

    /// Opens every input again for a new stream, and returns the reader it
    /// plays into its blocks. The inputs an older stream read close first:
    /// WinMM lets one opener at a time read a device.
    pub(crate) fn reader(&mut self) -> InputsReader {
        drop(self.inputs.take());
        let (mut inputs, reader) = hapto_midi_devices::inputs(ROOM);
        for device in self.devices.inputs() {
            open(&mut inputs, device, self.name);
        }
        self.inputs = Some(inputs);
        reader
    }

    /// Follows the system's list: opens the inputs that came and closes the
    /// ones that went or left. Call it once per tick.
    pub(crate) fn tick(&mut self, now: std::time::Instant) {
        let changes = self.devices.tick(now);
        let Some(inputs) = &mut self.inputs else {
            return;
        };
        for device in inputs.tick() {
            eprintln!("{}: MIDI in from {} left", self.name, device.name());
        }
        for device in &changes.removed {
            if inputs.close(device.id()) {
                eprintln!("{}: MIDI in from {} left", self.name, device.name());
            }
        }
        for device in &changes.added {
            if device.direction() == Direction::Input {
                open(inputs, device, self.name);
            }
        }
    }
}

/// Opens `device` and says so, or says why it did not open.
fn open(inputs: &mut Inputs, device: &Device, name: &str) {
    match inputs.open(device) {
        Ok(()) => eprintln!("{name}: MIDI in from {}", device.name()),
        Err(why) => eprintln!("{name}: MIDI in from {} did not open: {why}", device.name()),
    }
}
