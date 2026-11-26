//! Tests that the audio thread's sides of the app's MIDI neither allocate
//! nor free:
//!
//! - Every input's packets read into a block, merged in time order and
//!   upgraded, with a SysEx held until whole, and the pushes that feed them.
//! - Each block's MIDI downgraded and handed to every output that is read.

hapto_testkit::install_alloc_gate!();

use hapto_midi_devices::{Instant, MAX_INPUTS, MAX_OUTPUTS, inputs, outputs};
use hapto_testkit::alloc::assert_no_allocations;
use hapto_ump::{Event, EventList};

#[test]
fn reading_every_input_into_blocks_does_not_allocate() {
    let (mut inputs, mut reader) = inputs(64);
    let mut block = EventList::with_capacity(256);
    let seen = assert_no_allocations("inputs read into blocks", || {
        let mut seen = 0;
        for round in 0..8_u64 {
            for input in 0..MAX_INPUTS {
                let at = Instant::from_nanos(round * 1_000 + input as u64);
                let note = [0x2090_3C7F, 0, 0, 0];
                // The first input also gets a SysEx in two halves, which
                // waits a block for its end and then goes out whole.
                match (input, round % 2) {
                    (0, 0) => {
                        inputs.push(input, at, note);
                        inputs.push(input, at, [0x3016_0101, 0, 0, 0]);
                    }
                    (0, _) => {
                        inputs.push(input, at, [0x3032_0101, 0, 0, 0]);
                        inputs.push(input, at, note);
                    }
                    _ => {
                        inputs.push(input, at, note);
                    }
                }
            }
            block.clear();
            reader.read(Instant::from_nanos(round * 1_000 + 999), 128, &mut block);
            seen += block.as_slice().len();
        }
        seen
    });
    // Every note, and each SysEx's two halves once whole.
    assert_eq!(seen, 8 * MAX_INPUTS + 8, "every packet, every block");
    assert_eq!(reader.dropped(), 0);
}

#[test]
fn writing_blocks_to_every_output_does_not_allocate() {
    let (mut outputs, mut writer) = outputs(64);
    // Every output is read, as open devices read them.
    for output in 0..MAX_OUTPUTS {
        outputs.take(output);
    }
    // A note, a registered controller (sent as four controllers) and a SysEx.
    let block = [
        Event::new(0, [0x4090_3C00, 0xFFFF_0000, 0, 0]),
        Event::new(4, [0x4020_0000, 0x1800_0000, 0, 0]),
        Event::new(8, [0x3016_0102, 0x0304_0506, 0, 0]),
        Event::new(8, [0x3032_0708, 0, 0, 0]),
    ];
    assert_no_allocations("blocks written to every output", || {
        for round in 0..4_u64 {
            writer.write(Instant::from_nanos(round * 1_000_000), 48_000.0, &block);
        }
    });
    let taken = outputs.take(0).map(|taken| taken.len());
    assert_eq!(taken, Some(4 * 7), "seven packets a block");
    assert_eq!((writer.dropped(), writer.lost()), (0, 0));
}
