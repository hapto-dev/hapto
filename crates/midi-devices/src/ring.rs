//! A ring of timestamped packets with one producer and one consumer, between
//! a device's thread and the audio thread.
//!
//! Its slots are atomics, so neither side waits, locks or allocates, and no
//! `unsafe` is needed. An input's device pushes and the audio thread takes,
//! and an output's audio thread pushes and its device takes.

// A panic on the audio path ends the app's audio, so indexing, unwrap,
// expect, panic and unchecked arithmetic are denied. A provably safe site
// gives its reason in an `#[expect]`.
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

/// One packet and its timestamp.
struct Slot {
    at: AtomicU64,
    words: [AtomicU32; 4],
}

struct Ring {
    slots: Box<[Slot]>,
    /// The slot count minus one. The count is a power of two.
    mask: usize,
    /// Packets written so far, wrapping. Only the producer stores it.
    head: AtomicUsize,
    /// Packets taken so far, wrapping. Only the consumer stores it.
    tail: AtomicUsize,
    /// Packets dropped for lack of room, counted only by the producer.
    no_room: AtomicU64,
    /// Messages that the consumer dropped itself, which only it counts.
    consumer_dropped: AtomicU64,
    /// How many devices have pushed into the ring. A new device starts the
    /// stream over.
    generation: AtomicU32,
    /// Whether a device takes from the ring now. An output's producer pushes
    /// only then.
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
        no_room: AtomicU64::new(0),
        consumer_dropped: AtomicU64::new(0),
        generation: AtomicU32::new(0),
        reading: AtomicBool::new(false),
    });
    (Producer(Arc::clone(&ring)), Consumer(ring))
}

/// The pushing end, used by an input's device or an output's audio thread.
pub(crate) struct Producer(Arc<Ring>);

impl Producer {
    /// Adds a packet stamped `at`, or drops and counts it if the ring is full.
    ///
    /// Thread: the producer's. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub(crate) fn push(&mut self, at: Instant, words: [u32; 4]) {
        let ring = &*self.0;
        let head = ring.head.load(Ordering::Relaxed);
        // Acquire, so the consumer has finished reading every slot below the
        // tail it stored, and writing one can't tear its read.
        let tail = ring.tail.load(Ordering::Acquire);
        let Some(slot) = ring.slots.get(head & ring.mask) else {
            return;
        };
        if head.wrapping_sub(tail) > ring.mask {
            ring.no_room.fetch_add(1, Ordering::Relaxed);
            return;
        }
        slot.at.store(at.nanos(), Ordering::Relaxed);
        for (word, &value) in slot.words.iter().zip(&words) {
            word.store(value, Ordering::Relaxed);
        }
        // Release, so a consumer that sees the new head also sees the slot.
        ring.head.store(head.wrapping_add(1), Ordering::Release);
    }

    /// Marks that a new device pushes from now on, so the consumer starts its
    /// stream over.
    ///
    /// Thread: main, before the device gets the producer.
    pub(crate) fn renew(&mut self) {
        // Release, so a consumer that reads the new device's packet also reads
        // the new generation.
        self.0.generation.fetch_add(1, Ordering::Release);
    }

    /// Returns whether a device takes from the ring now.
    ///
    /// Thread: the producer's.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub(crate) fn is_read(&self) -> bool {
        self.0.reading.load(Ordering::Acquire)
    }

    /// Returns how many packets have been dropped, counted as in
    /// [`Consumer::dropped`].
    pub(crate) fn dropped(&self) -> u64 {
        self.0.dropped()
    }
}

impl Ring {
    fn dropped(&self) -> u64 {
        let no_room = self.no_room.load(Ordering::Relaxed);
        no_room.saturating_add(self.consumer_dropped.load(Ordering::Relaxed))
    }
}

/// The taking end, used by the audio thread for an input and by the device
/// for an output.
pub(crate) struct Consumer(Arc<Ring>);

impl Consumer {
    /// Sets whether a device takes from the ring from now on.
    pub(crate) fn set_reading(&self, reading: bool) {
        // Release, so a producer that sees the flag set sees the ring as the
        // device found it.
        self.0.reading.store(reading, Ordering::Release);
    }

    /// Marks the ring as read until the returned guard drops.
    ///
    /// A device's thread holds the guard, so the mark lasts until the thread
    /// ends, however it ends, and the writer never pushes into a ring that
    /// nobody reads.
    pub(crate) fn reading(&self) -> Reading {
        self.set_reading(true);
        Reading(Arc::clone(&self.0))
    }

    /// Discards every waiting packet, because a device doesn't send what was
    /// pushed before it took the ring.
    pub(crate) fn clear(&mut self) {
        let ring = &*self.0;
        // Acquire and release as in `pop`, for every slot at once.
        let head = ring.head.load(Ordering::Acquire);
        ring.tail.store(head, Ordering::Release);
    }

    /// Counts a message that the consumer dropped itself, such as one that
    /// the device refused or a SysEx that it couldn't send whole.
    ///
    /// [`dropped`](Self::dropped) includes these.
    #[cfg_attr(
        all(not(hapto_backend), not(test)),
        expect(
            dead_code,
            reason = "only a backend's output counts what it drops, and this system has none"
        )
    )]
    pub(crate) fn count_dropped(&self) {
        self.0.consumer_dropped.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns how many packets the producer has dropped for lack of room.
    ///
    /// A SysEx under way during one of these drops may have lost a packet to
    /// it.
    #[cfg_attr(
        all(not(hapto_backend), not(test)),
        expect(
            dead_code,
            reason = "only a backend's output joins a SysEx, and this system has none"
        )
    )]
    pub(crate) fn no_room(&self) -> u64 {
        self.0.no_room.load(Ordering::Relaxed)
    }

    /// Returns the oldest packet not taken yet, with its timestamp.
    ///
    /// Thread: the consumer's.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub(crate) fn peek(&self) -> Option<(Instant, [u32; 4])> {
        self.peek_at(0)
    }

    /// Returns the packet `index` places after the oldest, with its timestamp,
    /// or `None` past the last.
    ///
    /// Thread: the consumer's.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub(crate) fn peek_at(&self, index: usize) -> Option<(Instant, [u32; 4])> {
        if index >= self.len() {
            return None;
        }
        let ring = &*self.0;
        let at = ring.tail.load(Ordering::Relaxed).wrapping_add(index);
        let slot = ring.slots.get(at & ring.mask)?;
        let at = Instant::from_nanos(slot.at.load(Ordering::Relaxed));
        let words = slot
            .words
            .each_ref()
            .map(|word| word.load(Ordering::Relaxed));
        Some((at, words))
    }

    /// Takes the packet that [`peek`](Self::peek) returns, or returns `None`
    /// if the ring is empty.
    ///
    /// Thread: the consumer's.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub(crate) fn pop(&mut self) -> Option<(Instant, [u32; 4])> {
        let packet = self.peek()?;
        let ring = &*self.0;
        let tail = ring.tail.load(Ordering::Relaxed);
        // Release, after the read, because a producer that sees the new tail
        // may reuse the slot.
        ring.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(packet)
    }

    /// Returns how many packets wait to be taken.
    ///
    /// Thread: the consumer's.
    pub(crate) fn len(&self) -> usize {
        let ring = &*self.0;
        let tail = ring.tail.load(Ordering::Relaxed);
        // Acquire pairs with the producer's release of the head, so every
        // packet counted can be read.
        ring.head.load(Ordering::Acquire).wrapping_sub(tail)
    }

    /// Returns how many packets the ring holds at most.
    pub(crate) fn capacity(&self) -> usize {
        self.0.slots.len()
    }

    /// Returns how many devices have pushed into the ring, which changes
    /// when a new one takes it.
    ///
    /// Thread: the consumer's.
    pub(crate) fn generation(&self) -> u32 {
        self.0.generation.load(Ordering::Acquire)
    }

    /// Returns how many packets found no room, plus how many messages the
    /// consumer dropped itself.
    pub(crate) fn dropped(&self) -> u64 {
        self.0.dropped()
    }
}

/// The guard that [`Consumer::reading`] returns, which marks the ring as
/// read until it drops.
pub(crate) struct Reading(Arc<Ring>);

impl Drop for Reading {
    fn drop(&mut self) {
        self.0.reading.store(false, Ordering::Release);
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
        while let Some((at, words)) = consumer.pop() {
            taken.push((at.nanos(), words[0] & 0x7F));
        }
        assert_eq!(taken, [(0, 0), (1, 1), (2, 2), (3, 3)]);
        assert_eq!(consumer.dropped(), 2);
        // Once taken, the room is free again, round the end of the slots.
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
    fn what_found_no_room_is_counted_apart_from_what_the_consumer_dropped() {
        let (mut producer, consumer) = ring(1);
        producer.push(Instant::from_nanos(0), packet(0));
        producer.push(Instant::from_nanos(1), packet(1));
        consumer.count_dropped();
        assert_eq!((consumer.no_room(), consumer.dropped()), (1, 2));
        assert_eq!(producer.dropped(), 2);
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
    fn a_ring_is_read_while_its_guard_lives_however_its_thread_ends() {
        let (producer, consumer) = ring(2);
        let reading = consumer.reading();
        assert!(producer.is_read());
        drop(reading);
        assert!(!producer.is_read());
        let failed = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let _reading = consumer.reading();
                    panic!("the device's thread fails");
                })
                .join()
        });
        assert!(failed.is_err());
        assert!(!producer.is_read(), "let go as the thread unwound");
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

    /// In every interleaving of pushes and takes, the audio thread takes a
    /// prefix of what was pushed, in order and whole, and the rest is counted
    /// as dropped.
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

    /// A clear racing the audio thread's pushes leaves whole packets, in the
    /// order they were pushed.
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

    /// While the device pushes more, every packet of a held SysEx that `len`
    /// counts reads whole and in order by index.
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

    /// A reader that finds a new device's packet and then reads the generation
    /// sees the new one, so it starts the device's translation over first.
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
