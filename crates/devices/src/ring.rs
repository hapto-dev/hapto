//! The hand-off from a device's thread to the audio thread: a ring of
//! stamped packets with one producer and one consumer, whose slots are
//! atomics, so neither side waits, locks or allocates and no `unsafe` is
//! needed.

// Audio-path code: a panic here ends the app's audio, so indexing, unwrap,
// expect, panic and unchecked arithmetic are refused, and a site that is
// provably safe says why in an `#[expect]`.
#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects
    )
)]

#[cfg(loom)]
use loom::sync::Arc;
#[cfg(loom)]
use loom::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
#[cfg(not(loom))]
use std::sync::Arc;
#[cfg(not(loom))]
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::Instant;

/// One packet and when it came.
struct Slot {
    at: AtomicU64,
    words: [AtomicU32; 4],
}

struct Ring {
    slots: Box<[Slot]>,
    /// The slot count less one: the count is a power of two.
    mask: usize,
    /// How many packets the producer has written, wrapping. Only it stores.
    head: AtomicUsize,
    /// How many the consumer has taken, wrapping. Only it stores.
    tail: AtomicUsize,
    /// How many packets found no room.
    dropped: AtomicU64,
}

/// Makes a ring with room for at least `room` packets, and at least one.
pub(crate) fn ring(room: usize) -> (Producer, Consumer) {
    let count = room.max(1).next_power_of_two();
    let slots = (0..count)
        .map(|_| Slot {
            at: AtomicU64::new(0),
            words: std::array::from_fn(|_| AtomicU32::new(0)),
        })
        .collect();
    let ring = Arc::new(Ring {
        slots,
        mask: count.saturating_sub(1),
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
        dropped: AtomicU64::new(0),
    });
    (Producer(Arc::clone(&ring)), Consumer(ring))
}

/// The device's side: it pushes packets as they come.
pub(crate) struct Producer(Arc<Ring>);

impl Producer {
    /// Adds a packet that came at `at`. With no room it is dropped and
    /// counted. Thread: the device's. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub(crate) fn push(&mut self, at: Instant, words: [u32; 4]) {
        let ring = &*self.0;
        let head = ring.head.load(Ordering::Relaxed);
        // Acquire: the consumer has read every slot below the tail it
        // stored, so writing one of them cannot tear its read.
        let tail = ring.tail.load(Ordering::Acquire);
        let Some(slot) = ring.slots.get(head & ring.mask) else {
            return;
        };
        if head.wrapping_sub(tail) > ring.mask {
            ring.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        slot.at.store(at.nanos(), Ordering::Relaxed);
        for (word, &value) in slot.words.iter().zip(&words) {
            word.store(value, Ordering::Relaxed);
        }
        // Release: the consumer that sees the new head sees the slot.
        ring.head.store(head.wrapping_add(1), Ordering::Release);
    }
}

/// The audio thread's side: it takes packets in the order they came.
pub(crate) struct Consumer(Arc<Ring>);

impl Consumer {
    /// The oldest packet not taken yet, and when it came. Thread: audio.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub(crate) fn peek(&self) -> Option<(Instant, [u32; 4])> {
        let ring = &*self.0;
        let tail = ring.tail.load(Ordering::Relaxed);
        // Acquire: pairs with the producer's release of the head.
        if ring.head.load(Ordering::Acquire) == tail {
            return None;
        }
        let slot = ring.slots.get(tail & ring.mask)?;
        let at = Instant::from_nanos(slot.at.load(Ordering::Relaxed));
        let words = std::array::from_fn(|index| {
            slot.words
                .get(index)
                .map_or(0, |word| word.load(Ordering::Relaxed))
        });
        Some((at, words))
    }

    /// Takes the packet [`peek`](Self::peek) showed. Thread: audio.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub(crate) fn pop(&mut self) {
        let ring = &*self.0;
        let tail = ring.tail.load(Ordering::Relaxed);
        if ring.head.load(Ordering::Acquire) != tail {
            // Release: the producer that sees the new tail may reuse the
            // slot, which this side is done reading.
            ring.tail.store(tail.wrapping_add(1), Ordering::Release);
        }
    }

    /// How many packets found no room, since the ring was made.
    pub(crate) fn dropped(&self) -> u64 {
        self.0.dropped.load(Ordering::Relaxed)
    }
}

#[cfg(all(test, not(loom)))]
mod tests {
    use super::*;

    fn packet(tag: u32) -> [u32; 4] {
        [0x2090_3C00 | tag, 0, 0, 0]
    }

    #[test]
    fn packets_come_out_in_order_and_past_the_room_are_counted() {
        let (mut producer, mut consumer) = ring(3);
        for tag in 0..6 {
            producer.push(Instant::from_nanos(u64::from(tag)), packet(tag));
        }
        // Room for 3 rounds up to 4.
        let mut taken = Vec::new();
        while let Some((at, words)) = consumer.peek() {
            taken.push((at.nanos(), words[0] & 0x7F));
            consumer.pop();
        }
        assert_eq!(taken, [(0, 0), (1, 1), (2, 2), (3, 3)]);
        assert_eq!(consumer.dropped(), 2);
        // Taken, the room is free again, round the end of the slots.
        for tag in 10..14 {
            producer.push(Instant::from_nanos(u64::from(tag)), packet(tag));
        }
        let mut again = 0;
        while consumer.peek().is_some() {
            consumer.pop();
            again += 1;
        }
        assert_eq!((again, consumer.dropped()), (4, 2));
    }

    #[test]
    fn a_pop_with_nothing_there_takes_nothing() {
        let (mut producer, mut consumer) = ring(2);
        consumer.pop();
        producer.push(Instant::from_nanos(7), packet(1));
        assert_eq!(consumer.peek().map(|(at, _)| at.nanos()), Some(7));
    }
}

#[cfg(all(test, loom))]
mod loom_models {
    use super::*;

    /// A device's thread pushes while the audio thread takes. Whatever the
    /// interleaving, the audio thread takes a prefix of what was pushed, in
    /// order and whole, and what found no room is counted.
    #[test]
    fn loom_the_audio_thread_takes_what_the_device_pushed_in_order_and_whole() {
        loom::model(|| {
            let (mut producer, mut consumer) = ring(2);
            let device = loom::thread::spawn(move || {
                for tag in 1..=3_u32 {
                    producer.push(Instant::from_nanos(u64::from(tag)), [tag, tag, tag, tag]);
                }
            });
            let mut taken = Vec::new();
            for _ in 0..3 {
                if let Some((at, words)) = consumer.peek() {
                    assert_eq!(words, [words[0]; 4], "a packet is never torn");
                    assert_eq!(at.nanos(), u64::from(words[0]));
                    taken.push(words[0]);
                    consumer.pop();
                }
            }
            device.join().expect("the device's thread");
            while let Some((_, words)) = consumer.peek() {
                taken.push(words[0]);
                consumer.pop();
            }
            assert!(taken.windows(2).all(|pair| pair[0] < pair[1]), "{taken:?}");
            let dropped = usize::try_from(consumer.dropped()).expect("small");
            assert_eq!(taken.len() + dropped, 3);
        });
    }
}
