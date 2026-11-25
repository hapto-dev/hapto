//! A SysEx joined from an output's ring, for its thread to send whole.

use hapto_ump::{Event, SysExJoin};

/// The longest SysEx an output sends whole; a longer one is dropped and
/// counted.
pub(crate) const SYSEX_BYTES: usize = 64 * 1024;

/// A SysEx being joined from the packets an output's ring hands over.
///
/// It joins every packet, so a message on a SysEx's group ends it. One
/// that lost a packet to a full ring, or came on another group while one
/// was under way, is dropped and counted rather than sent with a hole.
pub(crate) struct Sysex {
    /// The SysEx to send, framed, once [`join`](Self::join) says it is
    /// whole. Its room is made once and never grows, so a system may read
    /// it in place while it sends.
    pub(crate) bytes: Vec<u8>,
    join: SysExJoin,
    /// The packets the ring had dropped as the SysEx under way began, those
    /// this output counted itself left out: one more since lost a packet of
    /// it.
    ring_dropped_at_start: u64,
    /// The drops this output counted on the ring itself.
    counted: u64,
}

/// What one packet came to.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Joined {
    /// Whether a SysEx is whole in [`Sysex::bytes`], to send.
    pub(crate) whole: bool,
    /// How many messages it dropped, which the caller counts on the ring.
    pub(crate) dropped: u32,
}

impl Sysex {
    pub(crate) fn new() -> Sysex {
        Sysex {
            bytes: Vec::with_capacity(SYSEX_BYTES),
            join: SysExJoin::with_room(SYSEX_BYTES.saturating_sub(2)),
            ring_dropped_at_start: 0,
            counted: 0,
        }
    }

    /// Reads one packet, `ring_dropped` the ring's dropped count now. A
    /// SysEx whole goes into [`bytes`](Self::bytes), unless the ring dropped
    /// a packet while it came, maybe one of its.
    pub(crate) fn join(&mut self, event: &Event, ring_dropped: u64) -> Joined {
        let from_ring = ring_dropped.saturating_sub(self.counted);
        if let Some(packet) = event.sysex7()
            && packet.form.begins()
            && self
                .join
                .under_way()
                .is_none_or(|group| group == packet.group)
        {
            self.ring_dropped_at_start = from_ring;
        }
        let joined = self.join.push(event);
        let mut dropped = joined.dropped;
        let mut whole = false;
        if let Some(message) = joined.whole {
            let framed = message.framed();
            // A buffer a hung driver kept has no room left: nothing fits.
            if from_ring != self.ring_dropped_at_start || framed.len() > self.bytes.capacity() {
                dropped = dropped.saturating_add(1);
            } else {
                self.bytes.clear();
                self.bytes.extend_from_slice(framed);
                whole = true;
            }
        }
        self.counted = self.counted.saturating_add(u64::from(dropped));
        Joined { whole, dropped }
    }

    /// Notes a drop the output counted on the ring itself, for a message
    /// its device refused, so it is not taken for a packet the ring lost.
    pub(crate) fn count(&mut self) {
        self.counted = self.counted.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use hapto_ump::SysEx7;
    use hapto_ump::SysExForm::{self, Complete, Continue, End, Start};

    use super::*;

    /// Joins a SysEx packet on group 0, the ring having dropped nothing.
    fn join(sysex: &mut Sysex, form: SysExForm, bytes: &[u8]) -> Joined {
        join_on(sysex, 0, form, bytes, 0)
    }

    /// Joins a SysEx packet on `group`, the ring's dropped count at
    /// `dropped`.
    fn join_on(
        sysex: &mut Sysex,
        group: u8,
        form: SysExForm,
        bytes: &[u8],
        dropped: u64,
    ) -> Joined {
        sysex.join(&SysEx7::new(group, form, bytes).event(0), dropped)
    }

    const PART: Joined = Joined {
        whole: false,
        dropped: 0,
    };
    const WHOLE: Joined = Joined {
        whole: true,
        dropped: 0,
    };
    const DROPPED: Joined = Joined {
        whole: false,
        dropped: 1,
    };

    #[test]
    fn a_sysex_is_joined_whole_from_its_packets() {
        let mut sysex = Sysex::new();
        assert_eq!(join(&mut sysex, Complete, &[0x7E, 0x7F]), WHOLE);
        assert_eq!(sysex.bytes, [0xF0, 0x7E, 0x7F, 0xF7]);
        assert_eq!(join(&mut sysex, Start, &[1, 2, 3, 4, 5, 6]), PART);
        assert_eq!(join(&mut sysex, Continue, &[7, 8, 9, 10, 11, 12]), PART);
        assert_eq!(join(&mut sysex, End, &[13]), WHOLE);
        let expected: Vec<u8> = [0xF0].into_iter().chain(1..=13).chain([0xF7]).collect();
        assert_eq!(sysex.bytes, expected);
    }

    #[test]
    fn a_sysex_begun_before_the_output_opened_or_past_its_room_is_dropped() {
        let mut sysex = Sysex::new();
        // Its start was sent before the output opened.
        assert_eq!(join(&mut sysex, Continue, &[1, 2, 3]), PART);
        assert_eq!(join(&mut sysex, End, &[4]), DROPPED);
        // One longer than the room.
        assert_eq!(join(&mut sysex, Start, &[1; 6]), PART);
        for _ in 0..SYSEX_BYTES / 6 {
            join(&mut sysex, Continue, &[2; 6]);
        }
        assert_eq!(join(&mut sysex, End, &[3]), DROPPED);
        // The next goes whole again.
        assert_eq!(join(&mut sysex, Complete, &[9]), WHOLE);
        assert_eq!(sysex.bytes, [0xF0, 9, 0xF7]);
    }

    #[test]
    fn a_sysex_that_lost_a_packet_to_a_full_ring_is_dropped() {
        let mut sysex = Sysex::new();
        assert_eq!(join_on(&mut sysex, 0, Start, &[1; 6], 4), PART);
        assert_eq!(join_on(&mut sysex, 0, Continue, &[2; 6], 4), PART);
        // The ring dropped a packet while it came.
        assert_eq!(join_on(&mut sysex, 0, End, &[3], 5), DROPPED);
        // The next, with nothing dropped meanwhile but the one counted, goes
        // whole.
        assert_eq!(join_on(&mut sysex, 0, Complete, &[9], 6), WHOLE);
    }

    #[test]
    fn another_groups_sysex_while_one_is_under_way_is_dropped_and_the_first_goes_on() {
        let mut sysex = Sysex::new();
        assert_eq!(join_on(&mut sysex, 0, Start, &[1; 6], 0), PART);
        assert_eq!(join_on(&mut sysex, 1, Complete, &[7], 0), DROPPED);
        // The one under way goes on whole: the drop counted on the ring was
        // this output's own, not a packet lost.
        assert_eq!(join_on(&mut sysex, 0, End, &[2], 1), WHOLE);
        let expected: Vec<u8> = [0xF0].into_iter().chain([1; 6]).chain([2, 0xF7]).collect();
        assert_eq!(sysex.bytes, expected);
    }

    #[test]
    fn a_drop_the_output_counted_itself_leaves_a_sysex_under_way_whole() {
        let mut sysex = Sysex::new();
        assert_eq!(join_on(&mut sysex, 0, Start, &[1; 6], 0), PART);
        // A note on another group, which the device refused.
        sysex.count();
        assert_eq!(join_on(&mut sysex, 0, End, &[2], 1), WHOLE);
    }

    #[test]
    fn a_message_on_its_group_ends_a_sysex_under_way() {
        let mut sysex = Sysex::new();
        assert_eq!(join(&mut sysex, Start, &[1; 6]), PART);
        assert_eq!(
            sysex.join(&Event::new(0, [0x2090_3C7F, 0, 0, 0]), 0),
            DROPPED
        );
        // The rest of it passes unread, and the next goes whole.
        assert_eq!(join_on(&mut sysex, 0, End, &[2], 1), PART);
        assert_eq!(join_on(&mut sysex, 0, Complete, &[9], 1), WHOLE);
    }
}
