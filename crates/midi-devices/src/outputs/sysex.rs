//! A SysEx joined from an output's ring, for its thread to send whole.

use hapto_ump::{Event, SysExJoin};

use crate::ring::Consumer;

/// The longest SysEx an output sends whole; a longer one is dropped and
/// counted.
pub(crate) const SYSEX_BYTES: usize = 64 * 1024;

/// A SysEx being joined from the packets an output's ring hands over.
///
/// Every packet is joined, not only a SysEx's, so a message on a SysEx's
/// group ends that SysEx, as [`SysExJoin`] says. A SysEx that lost a packet
/// to a full ring is dropped and counted rather than sent with a hole.
pub(crate) struct OutgoingSysex {
    /// The SysEx to send, framed, once [`join`](Self::join) says it is
    /// whole. Its room is made once and never grows, so the system's API
    /// may read it in place while it sends.
    pub(crate) bytes: Vec<u8>,
    join: SysExJoin,
    /// The packets the ring had lost for want of room as the SysEx under
    /// way began: one more lost since may have been one of its own.
    lost_at_start: u64,
}

impl OutgoingSysex {
    pub(crate) fn new() -> OutgoingSysex {
        OutgoingSysex {
            bytes: Vec::with_capacity(SYSEX_BYTES),
            join: SysExJoin::with_room(SYSEX_BYTES.saturating_sub(2)),
            lost_at_start: 0,
        }
    }

    /// Joins one packet `ring` handed over, and says whether a SysEx is now
    /// whole in [`bytes`](Self::bytes), to send. A message it drops, it
    /// counts on the ring.
    pub(crate) fn join(&mut self, event: &Event, ring: &Consumer) -> bool {
        let lost = ring.no_room();
        if let Some(packet) = event.sysex7()
            && packet.form.begins()
            && self
                .join
                .under_way()
                .is_none_or(|group| group == packet.group)
        {
            self.lost_at_start = lost;
        }
        let joined = self.join.push(event);
        let mut dropped = joined.dropped;
        let mut whole = false;
        if let Some(message) = joined.whole {
            let framed = message.framed();
            // WinMM leaves the bytes to a driver that hung while sending
            // them, which leaves no room: nothing fits from then on.
            if lost != self.lost_at_start || framed.len() > self.bytes.capacity() {
                dropped = dropped.saturating_add(1);
            } else {
                self.bytes.clear();
                self.bytes.extend_from_slice(framed);
                whole = true;
            }
        }
        for _ in 0..dropped {
            ring.count_dropped();
        }
        whole
    }
}

#[cfg(test)]
mod tests {
    use hapto_ump::SysEx7;
    use hapto_ump::SysExForm::{self, Complete, Continue, End, Start};

    use super::*;
    use crate::Instant;
    use crate::ring::{Producer, ring};

    /// A ring kept full, so the producer loses each packet it pushes.
    fn full_ring() -> (Producer, Consumer) {
        let (mut producer, consumer) = ring(1);
        producer.push(Instant::from_nanos(0), [0; 4]);
        (producer, consumer)
    }

    /// Loses a packet to the full ring.
    fn lose_a_packet(producer: &mut Producer) {
        producer.push(Instant::from_nanos(0), [0; 4]);
    }

    /// Joins a SysEx packet on `group`.
    fn join_on(
        sysex: &mut OutgoingSysex,
        ring: &Consumer,
        group: u8,
        form: SysExForm,
        bytes: &[u8],
    ) -> bool {
        sysex.join(&SysEx7::new(group, form, bytes).event(0), ring)
    }

    /// Joins a SysEx packet on group 0.
    fn join(sysex: &mut OutgoingSysex, ring: &Consumer, form: SysExForm, bytes: &[u8]) -> bool {
        join_on(sysex, ring, 0, form, bytes)
    }

    #[test]
    fn a_sysex_is_joined_whole_from_its_packets() {
        let (_producer, ring) = full_ring();
        let mut sysex = OutgoingSysex::new();
        assert!(join(&mut sysex, &ring, Complete, &[0x7E, 0x7F]));
        assert_eq!(sysex.bytes, [0xF0, 0x7E, 0x7F, 0xF7]);
        assert!(!join(&mut sysex, &ring, Start, &[1, 2, 3, 4, 5, 6]));
        assert!(!join(&mut sysex, &ring, Continue, &[7, 8, 9, 10, 11, 12]));
        assert!(join(&mut sysex, &ring, End, &[13]));
        let expected: Vec<u8> = [0xF0].into_iter().chain(1..=13).chain([0xF7]).collect();
        assert_eq!(sysex.bytes, expected);
        assert_eq!(ring.dropped(), 0);
    }

    #[test]
    fn a_sysex_begun_before_the_output_opened_or_past_its_room_is_dropped() {
        let (_producer, ring) = full_ring();
        let mut sysex = OutgoingSysex::new();
        // Its start was sent before the output opened.
        assert!(!join(&mut sysex, &ring, Continue, &[1, 2, 3]));
        assert!(!join(&mut sysex, &ring, End, &[4]));
        assert_eq!(ring.dropped(), 1);
        // One longer than the room.
        assert!(!join(&mut sysex, &ring, Start, &[1; 6]));
        for _ in 0..SYSEX_BYTES / 6 {
            join(&mut sysex, &ring, Continue, &[2; 6]);
        }
        assert!(!join(&mut sysex, &ring, End, &[3]));
        assert_eq!(ring.dropped(), 2);
        // The next goes whole again.
        assert!(join(&mut sysex, &ring, Complete, &[9]));
        assert_eq!(sysex.bytes, [0xF0, 9, 0xF7]);
    }

    #[test]
    fn a_sysex_that_lost_a_packet_to_a_full_ring_is_dropped() {
        let (mut producer, ring) = full_ring();
        let mut sysex = OutgoingSysex::new();
        assert!(!join(&mut sysex, &ring, Start, &[1; 6]));
        assert!(!join(&mut sysex, &ring, Continue, &[2; 6]));
        lose_a_packet(&mut producer);
        assert!(!join(&mut sysex, &ring, End, &[3]));
        // The packet lost, and the SysEx it may have been part of.
        assert_eq!(ring.dropped(), 2);
        // The next, with nothing lost meanwhile, goes whole.
        assert!(join(&mut sysex, &ring, Complete, &[9]));
    }

    #[test]
    fn another_groups_sysex_is_dropped_and_the_one_under_way_goes_on() {
        let (_producer, ring) = full_ring();
        let mut sysex = OutgoingSysex::new();
        assert!(!join_on(&mut sysex, &ring, 0, Start, &[1; 6]));
        assert!(!join_on(&mut sysex, &ring, 1, Complete, &[7]));
        assert_eq!(ring.dropped(), 1);
        // The one under way goes on whole: the drop counted on the ring was
        // this output's own, not a packet lost.
        assert!(join_on(&mut sysex, &ring, 0, End, &[2]));
        let expected: Vec<u8> = [0xF0].into_iter().chain([1; 6]).chain([2, 0xF7]).collect();
        assert_eq!(sysex.bytes, expected);
    }

    #[test]
    fn a_message_the_device_refused_leaves_a_sysex_under_way_whole() {
        let (_producer, ring) = full_ring();
        let mut sysex = OutgoingSysex::new();
        assert!(!join(&mut sysex, &ring, Start, &[1; 6]));
        // A note on another group, which the device refused.
        ring.count_dropped();
        assert!(join(&mut sysex, &ring, End, &[2]));
    }

    #[test]
    fn a_message_on_its_group_ends_a_sysex_under_way() {
        let (_producer, ring) = full_ring();
        let mut sysex = OutgoingSysex::new();
        assert!(!join(&mut sysex, &ring, Start, &[1; 6]));
        assert!(!sysex.join(&Event::new(0, [0x2090_3C7F, 0, 0, 0]), &ring));
        assert_eq!(ring.dropped(), 1);
        // The rest of it passes unread, and the next goes whole.
        assert!(!join(&mut sysex, &ring, End, &[2]));
        assert!(join(&mut sysex, &ring, Complete, &[9]));
    }
}
