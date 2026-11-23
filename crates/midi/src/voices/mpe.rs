//! MPE's zones, for a [`Poly`](super::Poly) that opts in to them.

use crate::values::{Channel, Group};

/// One group's MPE zones, each named by how many member channels it has; a
/// zone of none is off. The lower zone's master is channel 1, its members
/// the channels above; the upper zone's master is channel 16, its members
/// the channels below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Zones {
    lower: u8,
    upper: u8,
}

impl Zones {
    /// MPE's own default: a lower zone of every other channel.
    pub(super) const LOWER_15: Zones = Zones {
        lower: 15,
        upper: 0,
    };

    /// The master channel of the zone `channel` is a member of, if any.
    pub(super) fn master(self, channel: Channel) -> Option<Channel> {
        let index = channel.index();
        if (1..=usize::from(self.lower)).contains(&index) {
            Some(Channel::FIRST)
        } else if (15_usize.saturating_sub(usize::from(self.upper))..15).contains(&index) {
            Some(LAST)
        } else {
            None
        }
    }

    /// Sets the zone whose master is `channel` to `members` member channels,
    /// as an MPE Configuration Message does, shrinking the other zone if the
    /// two would overlap. The channels whose bend ranges return to MPE's
    /// defaults come back: the master, and its members; `None` for a channel
    /// that is no zone's master.
    pub(super) fn configure(
        &mut self,
        channel: Channel,
        members: u8,
    ) -> Option<impl Iterator<Item = Channel>> {
        let members = members.min(15);
        let room = 14_u8.saturating_sub(members);
        let indices = if channel == Channel::FIRST {
            self.lower = members;
            if members > 0 {
                self.upper = self.upper.min(room);
            }
            1..members.saturating_add(1)
        } else if channel == LAST {
            self.upper = members;
            if members > 0 {
                self.lower = self.lower.min(room);
            }
            15_u8.saturating_sub(members)..15
        } else {
            return None;
        };
        Some(indices.map(Channel::from_wire))
    }
}

const LAST: Channel = Channel::number(16);

/// Every group's zones, for a `Poly` playing MPE.
pub(super) type Mpe = [Zones; 16];

/// The master channel of the zone `channel` of `group` is a member of, when
/// MPE is on.
pub(super) fn master_of(mpe: Option<&Mpe>, group: Group, channel: Channel) -> Option<Channel> {
    mpe?.get(group.index())?.master(channel)
}

#[cfg(test)]
mod tests {
    use super::Zones;
    use crate::Channel;

    fn members(zones: Zones) -> Vec<(u8, Option<u8>)> {
        (1..=16)
            .map(|number| {
                let master = zones.master(Channel::number(number));
                (number, master.map(|channel| channel.wire() + 1))
            })
            .filter(|(_, master)| master.is_some())
            .collect()
    }

    #[test]
    fn the_default_zone_is_the_lower_with_every_other_channel() {
        let zones = members(Zones::LOWER_15);
        assert_eq!(zones.len(), 15);
        assert_eq!(zones.first(), Some(&(2, Some(1))));
        assert_eq!(zones.last(), Some(&(16, Some(1))));
    }

    #[test]
    fn a_configuration_message_shrinks_the_zone_it_would_overlap() {
        let mut zones = Zones::LOWER_15;
        let reset: Vec<u8> = zones
            .configure(Channel::number(16), 3)
            .expect("16 is the upper zone's master")
            .map(Channel::wire)
            .collect();
        assert_eq!(reset, [12, 13, 14], "channels 13 to 15");
        assert_eq!(zones.master(Channel::number(13)), Some(Channel::number(16)));
        assert_eq!(zones.master(Channel::number(12)), Some(Channel::FIRST));
        assert_eq!(zones.master(Channel::number(14)), Some(Channel::number(16)));
        assert_eq!(
            zones,
            Zones {
                lower: 11,
                upper: 3
            }
        );
        // A zone of no members is off.
        zones.configure(Channel::FIRST, 0);
        assert_eq!(zones.master(Channel::number(2)), None);
        assert!(zones.configure(Channel::number(5), 4).is_none());
    }
}
