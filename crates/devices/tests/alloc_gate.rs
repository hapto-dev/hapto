//! The audio thread's side of the app's MIDI in neither allocates nor
//! frees: every input's packets read into a block, merged in time order,
//! and the pushes that feed them.

hapto_testkit::install_alloc_gate!();

use hapto_devices::{GROUPS, Instant, inputs};
use hapto_testkit::alloc::assert_no_allocations;
use hapto_ump::EventList;

#[test]
fn reading_every_input_into_blocks_does_not_allocate() {
    let (mut inputs, mut reader) = inputs(64);
    let mut block = EventList::with_capacity(256);
    let groups = u8::try_from(GROUPS).expect("16");
    let seen = assert_no_allocations("inputs read into blocks", || {
        let mut seen = 0;
        for round in 0..8_u64 {
            for group in 0..groups {
                let at = Instant::from_nanos(round * 1_000 + u64::from(group));
                let note = 0x2090_3C7F | u32::from(group) << 24;
                inputs.push(group, at, [note, 0, 0, 0]);
            }
            block.clear();
            reader.read(Instant::from_nanos(round * 1_000 + 999), 128, &mut block);
            seen += block.as_slice().len();
        }
        seen
    });
    assert_eq!(seen, 8 * GROUPS, "every packet, every block");
    assert_eq!(reader.dropped(), 0);
}
