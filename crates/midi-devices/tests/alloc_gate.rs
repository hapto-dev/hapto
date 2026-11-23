//! The audio thread's side of the app's MIDI in neither allocates nor
//! frees: every input's packets read into a block, merged in time order and
//! upgraded, a SysEx held until whole, and the pushes that feed them.

hapto_testkit::install_alloc_gate!();

use hapto_midi_devices::{Instant, MAX_INPUTS, inputs};
use hapto_testkit::alloc::assert_no_allocations;
use hapto_ump::EventList;

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
                // On the first, a SysEx in two halves too, which waits a
                // block for its end and then goes whole.
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
