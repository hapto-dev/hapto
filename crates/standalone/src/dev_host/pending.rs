//! The parameter changes the dev host's settings window makes on the main
//! thread, for the next block on the audio thread, as a host's generic
//! controls send a VST3 processor its changes with the block. One slot per
//! parameter, the latest write winning, taken with no lock and no
//! allocation.

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicU64, Ordering};

/// A slot with no change: a NaN's bits, which no write stores, since a
/// write is clamped to 0..1.
const EMPTY: u64 = u64::MAX;

/// The changes waiting for the next block, by the parameter's index in the
/// host's list.
#[derive(Debug)]
pub(crate) struct Pending {
    ids: Vec<u32>,
    slots: Vec<AtomicU64>,
}

impl Pending {
    /// Holds a slot for each of the parameters whose host ids are `ids`, in
    /// the host's order.
    pub(crate) fn new(ids: Vec<u32>) -> Pending {
        let slots = ids.iter().map(|_| AtomicU64::new(EMPTY)).collect();
        Pending { ids, slots }
    }

    /// Asks the next block to set the parameter at `index` to `normalized`,
    /// clamped to 0..1, replacing a change still waiting for it. Any thread.
    pub(crate) fn set(&self, index: usize, normalized: f64) {
        if let Some(slot) = self.slots.get(index) {
            let value = if normalized.is_nan() {
                0.0
            } else {
                normalized.clamp(0.0, 1.0)
            };
            slot.store(value.to_bits(), Ordering::Release);
        }
    }

    /// Moves the waiting changes into `into`, as `(host id, 0..1)`, and
    /// returns how many. Those past its room wait for the next block.
    /// Thread: audio.
    pub(crate) fn take(&self, into: &mut [(u32, f64)]) -> usize {
        let mut count = 0;
        for (slot, &id) in self.slots.iter().zip(&self.ids) {
            let Some(room) = into.get_mut(count) else {
                break;
            };
            let bits = slot.swap(EMPTY, Ordering::AcqRel);
            if bits != EMPTY {
                *room = (id, f64::from_bits(bits));
                count += 1;
            }
        }
        count
    }
}
