//! The hand-off between a device's thread and the audio thread: a ring of
//! stamped packets with one producer and one consumer, whose slots are
//! atomics, so neither side waits, locks or allocates and no `unsafe` is
//! needed. An input's device pushes and the audio thread takes; an output's
//! audio thread pushes and its device takes.

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
use loom::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
#[cfg(not(loom))]
use std::sync::Arc;
#[cfg(not(loom))]
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

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
    /// Counts the devices that have pushed into the ring: a new one starts
    /// the stream over.
    generation: AtomicU32,
    /// Whether a device takes from the ring now: an output's producer
    /// pushes only while one does.
    reading: AtomicBool,
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
        generation: AtomicU32::new(0),
        reading: AtomicBool::new(false),
    });
    (Producer(Arc::clone(&ring)), Consumer(ring))
}

/// The pushing side: an input's device, as packets come, or an output's
/// audio thread, as a block sends them.
pub(crate) struct Producer(Arc<Ring>);

impl Producer {
    /// Adds a packet stamped `at`. With no room it is dropped and counted.
    /// Thread: the producer's. Allocates nothing.
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

    /// Says a new device pushes from now on, so the consumer starts its
    /// stream over. Thread: main, before the device is handed the producer.
    pub(crate) fn renew(&mut self) {
        // Release: the consumer that reads a packet the new device pushed
        // reads the new generation with it.
        self.0.generation.fetch_add(1, Ordering::Release);
    }

    /// Whether a device takes from the ring now. Thread: the producer's.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub(crate) fn is_read(&self) -> bool {
        self.0.reading.load(Ordering::Acquire)
    }

    /// How many packets found no room, since the ring was made.
    pub(crate) fn dropped(&self) -> u64 {
        self.0.dropped.load(Ordering::Relaxed)
    }
}

/// The taking side: the audio thread, for an input, or an output's device:
/// it takes packets in the order they were pushed.
pub(crate) struct Consumer(Arc<Ring>);

impl Consumer {
    /// Says whether a device takes from the ring from now on.
    pub(crate) fn set_reading(&self, reading: bool) {
        // Release: a producer that sees it set sees the ring as the
        // device found it.
        self.0.reading.store(reading, Ordering::Release);
    }

    /// Takes every packet waiting, unread: what was pushed before a device
    /// took the ring is not its to send.
    pub(crate) fn clear(&mut self) {
        let ring = &*self.0;
        // Acquire and release as in `pop`, for every slot at once.
        let head = ring.head.load(Ordering::Acquire);
        ring.tail.store(head, Ordering::Release);
    }

    /// Counts a message the consumer had no room for, as a full ring
    /// counts a packet.
    #[cfg_attr(
        all(not(windows), not(test)),
        expect(
            dead_code,
            reason = "only a backend's output counts what it drops, and this system has none"
        )
    )]
    pub(crate) fn count_dropped(&self) {
        self.0.dropped.fetch_add(1, Ordering::Relaxed);
    }

    /// The oldest packet not taken yet, and when it came. Thread: the
    /// consumer's.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub(crate) fn peek(&self) -> Option<(Instant, [u32; 4])> {
        self.peek_at(0)
    }

    /// The packet `index` places after the oldest not taken yet, and when
    /// it came: `None` past the last. Thread: the consumer's.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub(crate) fn peek_at(&self, index: usize) -> Option<(Instant, [u32; 4])> {
        if index >= self.len() {
            return None;
        }
        let ring = &*self.0;
        let at = ring.tail.load(Ordering::Relaxed).wrapping_add(index);
        let slot = ring.slots.get(at & ring.mask)?;
        let at = Instant::from_nanos(slot.at.load(Ordering::Relaxed));
        let words = std::array::from_fn(|index| {
            slot.words
                .get(index)
                .map_or(0, |word| word.load(Ordering::Relaxed))
        });
        Some((at, words))
    }

    /// Takes the packet [`peek`](Self::peek) showed. Thread: the consumer's.
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

    /// How many packets wait to be taken. Thread: the consumer's.
    pub(crate) fn len(&self) -> usize {
        let ring = &*self.0;
        let tail = ring.tail.load(Ordering::Relaxed);
        // Acquire: pairs with the producer's release of the head, so every
        // packet counted can be read.
        ring.head.load(Ordering::Acquire).wrapping_sub(tail)
    }

    /// How many packets the ring holds at most.
    pub(crate) fn capacity(&self) -> usize {
        self.0.slots.len()
    }

    /// How many devices have pushed into the ring: a change means a new
    /// one took it. Thread: the consumer's.
    pub(crate) fn generation(&self) -> u32 {
        self.0.generation.load(Ordering::Acquire)
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
    fn a_peek_reaches_every_packet_waiting_and_no_further() {
        let (mut producer, consumer) = ring(4);
        for tag in 0..3 {
            producer.push(Instant::from_nanos(u64::from(tag)), packet(tag));
        }
        let tags: Vec<u32> = (0..5)
            .filter_map(|index| consumer.peek_at(index))
            .map(|(_, words)| words[0] & 0x7F)
            .collect();
        assert_eq!(tags, [0, 1, 2]);
        assert_eq!((consumer.len(), consumer.capacity()), (3, 4));
        assert_eq!(consumer.generation(), 0);
        producer.renew();
        assert_eq!(consumer.generation(), 1);
    }

    #[test]
    fn a_clear_takes_every_packet_waiting_and_reading_is_told_across() {
        let (mut producer, mut consumer) = ring(4);
        assert!(!producer.is_read());
        consumer.set_reading(true);
        assert!(producer.is_read());
        for tag in 0..3 {
            producer.push(Instant::from_nanos(u64::from(tag)), packet(tag));
        }
        consumer.clear();
        assert_eq!(consumer.len(), 0);
        consumer.count_dropped();
        assert_eq!(consumer.dropped(), 1);
        producer.push(Instant::from_nanos(9), packet(9));
        assert_eq!(consumer.peek().map(|(at, _)| at.nanos()), Some(9));
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

    /// An output clears its ring as a device takes it, while the audio
    /// thread may push. Whatever the interleaving, what is left afterwards
    /// is whole packets, in the order they were pushed.
    #[test]
    fn loom_a_clear_racing_a_push_leaves_whole_packets_in_order() {
        loom::model(|| {
            let (mut producer, mut consumer) = ring(4);
            let audio = loom::thread::spawn(move || {
                for tag in 1..=2_u32 {
                    producer.push(Instant::from_nanos(u64::from(tag)), [tag; 4]);
                }
            });
            consumer.clear();
            audio.join().expect("the audio thread");
            let mut left = Vec::new();
            while let Some((at, words)) = consumer.peek() {
                assert_eq!(words, [words[0]; 4], "a packet is never torn");
                assert_eq!(at.nanos(), u64::from(words[0]));
                left.push(words[0]);
                consumer.pop();
            }
            assert!(left.windows(2).all(|pair| pair[0] < pair[1]), "{left:?}");
            assert!(left.len() <= 2);
        });
    }

    /// The reader looks over a SysEx held in the ring by its place, while
    /// the device pushes more. Every packet the length counts reads whole,
    /// in order.
    #[test]
    fn loom_every_packet_the_length_counts_reads_whole_by_its_place() {
        loom::model(|| {
            let (mut producer, consumer) = ring(4);
            let device = loom::thread::spawn(move || {
                for tag in 1..=2_u32 {
                    producer.push(Instant::from_nanos(u64::from(tag)), [tag; 4]);
                }
            });
            let counted = consumer.len();
            for index in 0..counted {
                let (_, words) = consumer.peek_at(index).expect("counted");
                assert_eq!(words, [u32::try_from(index + 1).expect("small"); 4]);
            }
            device.join().expect("the device's thread");
        });
    }

    /// A new device takes an input: the main thread renews the ring, and the
    /// device then pushes. A packet the reader finds came with the new
    /// generation, which it reads after the packet, so it starts the new
    /// device's translation over before it.
    #[test]
    fn loom_a_packet_a_new_device_pushed_comes_with_its_generation() {
        loom::model(|| {
            let (mut producer, consumer) = ring(2);
            let device = loom::thread::spawn(move || {
                producer.renew();
                producer.push(Instant::from_nanos(1), [1; 4]);
            });
            if consumer.peek().is_some() {
                assert_eq!(consumer.generation(), 1);
            }
            device.join().expect("the device's thread");
        });
    }
}
