//! A value for each MIDI channel.

use std::ops::{Index, IndexMut};

use crate::values::{Channel, Group};

/// A value for each of the 256 channels in a MIDI 2.0 stream, 16 in each
/// group.
///
/// MIDI 1.0's 16 are the first group's. A plugin keeps its per-channel state
/// in it, such as the bend, volume or pedals, indexed by a message's group
/// and channel.
///
/// # Examples
///
/// ```
/// use hapto_midi::controller::VOLUME;
/// use hapto_midi::{Channel, Decoder, Group, PerChannel};
/// use hapto_ump::Event;
///
/// let mut volume = PerChannel::new(VOLUME.default());
/// // Channel 2's volume all the way up.
/// let event = Event::new(0, [0x40B1_0700, u32::MAX, 0, 0]);
/// for message in Decoder::new().decode(&event) {
///     if let Some((group, channel, value)) = VOLUME.read_channel(&message) {
///         volume[(group, channel)] = value;
///     }
/// }
/// assert_eq!(volume[(Group::FIRST, Channel::number(2))], 1.0);
/// assert_eq!(volume[(Group::FIRST, Channel::FIRST)], VOLUME.default());
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct PerChannel<T>([T; 256]);

impl<T: Clone> PerChannel<T> {
    /// Returns a `PerChannel` with `value` for every channel.
    pub fn new(value: T) -> Self {
        PerChannel(std::array::from_fn(|_| value.clone()))
    }
}

impl<T: Default> Default for PerChannel<T> {
    fn default() -> Self {
        PerChannel(std::array::from_fn(|_| T::default()))
    }
}

impl<T> PerChannel<T> {
    /// Returns an iterator over every channel's value, from the first
    /// group's first channel.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.0.iter_mut()
    }
}

/// Where a group's channel sits: each is below 16, so the slot is below 256.
pub(crate) fn slot(group: Group, channel: Channel) -> usize {
    group.index().wrapping_mul(16).wrapping_add(channel.index())
}

impl<T> Index<(Group, Channel)> for PerChannel<T> {
    type Output = T;

    #[cfg_attr(
        not(test),
        expect(
            clippy::indexing_slicing,
            reason = "a group and a channel are each below 16, so their slot is below 256"
        )
    )]
    fn index(&self, (group, channel): (Group, Channel)) -> &T {
        &self.0[slot(group, channel)]
    }
}

impl<T> IndexMut<(Group, Channel)> for PerChannel<T> {
    #[cfg_attr(
        not(test),
        expect(
            clippy::indexing_slicing,
            reason = "a group and a channel are each below 16, so their slot is below 256"
        )
    )]
    fn index_mut(&mut self, (group, channel): (Group, Channel)) -> &mut T {
        &mut self.0[slot(group, channel)]
    }
}

#[cfg(test)]
mod tests {
    use super::PerChannel;
    use crate::{Channel, Group};

    #[test]
    fn each_group_and_channel_has_a_value_of_its_own() {
        let mut values = PerChannel::new(0_u32);
        for group in 1..=16 {
            for channel in 1..=16 {
                values[(Group::number(group), Channel::number(channel))] =
                    u32::from(group) * 100 + u32::from(channel);
            }
        }
        assert_eq!(values[(Group::number(16), Channel::number(16))], 1616);
        assert_eq!(values[(Group::number(2), Channel::number(1))], 201);
        assert_eq!(values.iter_mut().count(), 256);
    }
}
