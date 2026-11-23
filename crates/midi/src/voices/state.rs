//! What [`Poly`](super::Poly) keeps of each channel and each key between
//! messages: the sources a note's [`Expression`](super::Expression) reads.

use crate::channels::slot;
use crate::controller::{Address, Controller, ControllerValue, number};
use crate::message::NoteKey;
use crate::values::{Centred, Channel, Group, Key, Pitch};

/// Controllers by number, each unset until a message sets it, so a reader
/// falls back to the next source: a note's own to its channel's, a
/// channel's to the controller's default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Set<const N: usize> {
    /// Bit `i` says whether `values[i]` was set; `N` is at most 128.
    set: u128,
    values: [f32; N],
}

impl<const N: usize> Set<N> {
    pub(super) const EMPTY: Self = Set {
        set: 0,
        values: [0.0; N],
    };

    pub(super) fn get(&self, index: usize) -> Option<f32> {
        let bit = bit(index)?;
        if self.set & bit == 0 {
            return None;
        }
        self.values.get(index).copied()
    }

    pub(super) fn put(&mut self, index: usize, value: f32) {
        if let (Some(bit), Some(slot)) = (bit(index), self.values.get_mut(index)) {
            self.set |= bit;
            *slot = value;
        }
    }

    pub(super) fn clear(&mut self, index: usize) {
        if let Some(bit) = bit(index) {
            self.set &= !bit;
        }
    }
}

fn bit(index: usize) -> Option<u128> {
    1_u128.checked_shl(u32::try_from(index).ok()?)
}

/// What a channel's messages set for its notes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ChannelState {
    pub(super) bend: Centred,
    /// How many semitones a full bend moves.
    pub(super) bend_range: f32,
    /// How many semitones a note's own full bend moves, shared by every key
    /// on the channel.
    pub(super) note_bend_range: f32,
    pub(super) pressure: f32,
    /// Fine and coarse tuning together, in semitones.
    pub(super) fine: f32,
    pub(super) coarse: f32,
    pub(super) controls: Set<128>,
}

impl ChannelState {
    /// A channel no message has reached. The bend's range is MIDI 1.0's 2
    /// semitones. MIDI 2.0 gives a note's own bend no default range; 48 is
    /// MPE's for the notes of a zone, the one a sender of per-note bends
    /// most likely assumes.
    pub(super) const NEW: ChannelState = ChannelState {
        bend: Centred::CENTRE,
        bend_range: 2.0,
        note_bend_range: 48.0,
        pressure: 0.0,
        fine: 0.0,
        coarse: 0.0,
        controls: Set::EMPTY,
    };

    /// The channel's value of `controller`, or its default while unset.
    pub(super) fn read<T: ControllerValue>(&self, controller: Controller<T>) -> T {
        self.control(controller.address())
            .map_or(controller.default(), T::from_unit)
    }

    /// The channel's value of the controller at `address`, from 0 to 1, if
    /// one has arrived. Only control changes are kept.
    pub(super) fn control(&self, address: Address) -> Option<f32> {
        match address {
            Address::Control(number) => self.controls.get(usize::from(number)),
            _ => None,
        }
    }

    /// Sets what a control change sets. The channel mode messages, 120 and
    /// up, are actions, not values, and set nothing.
    pub(super) fn set_control(&mut self, controller: u8, value: f32) {
        if controller < number::ALL_SOUND_OFF {
            self.controls.put(usize::from(controller), value);
        }
    }
}

/// Every group's channels, boxed: they are too large for a stack.
#[derive(Debug)]
pub(super) struct Channels(Box<[ChannelState]>);

impl Channels {
    pub(super) fn new() -> Self {
        Channels(vec![ChannelState::NEW; 256].into_boxed_slice())
    }

    #[cfg_attr(
        not(test),
        expect(
            clippy::indexing_slicing,
            reason = "a slot is below 256, and there are 256 channels"
        )
    )]
    pub(super) fn of(&self, group: Group, channel: Channel) -> &ChannelState {
        &self.0[slot(group, channel)]
    }

    #[cfg_attr(
        not(test),
        expect(
            clippy::indexing_slicing,
            reason = "a slot is below 256, and there are 256 channels"
        )
    )]
    pub(super) fn of_mut(&mut self, group: Group, channel: Channel) -> &mut ChannelState {
        &mut self.0[slot(group, channel)]
    }
}

/// The per-note controllers MIDI 2.0 defines a meaning for, by the
/// registered per-note controller's number: the only ones a note keeps.
const NOTE_CONTROLS: [u8; 21] = [
    1, 2, 7, 8, 10, 11, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 91, 92, 93, 94, 95,
];

/// Where a note keeps controller `number`; `None` for one MIDI 2.0 leaves
/// undefined, or pitch, which has a place of its own.
pub(super) fn note_slot(number: u8) -> Option<usize> {
    NOTE_CONTROLS.iter().position(|&kept| kept == number)
}

/// A note's own controllers, which MIDI 2.0 keeps for its key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct NoteControls {
    /// A pitch of its own, which overrides its key's.
    pub(super) pitch: Option<Pitch>,
    pub(super) bend: Centred,
    pub(super) pressure: Option<f32>,
    pub(super) controls: Set<21>,
}

impl NoteControls {
    pub(super) const NEW: NoteControls = NoteControls {
        pitch: None,
        bend: Centred::CENTRE,
        pressure: None,
        controls: Set::EMPTY,
    };

    /// The note's own value of the control change `number`, if it has one.
    pub(super) fn control(&self, number: u8) -> Option<f32> {
        self.controls.get(note_slot(number)?)
    }
}

/// A key on a group's channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Place {
    pub(super) group: Group,
    pub(super) channel: Channel,
    pub(super) key: Key,
}

impl From<NoteKey> for Place {
    fn from(note: NoteKey) -> Self {
        Place {
            group: note.group,
            channel: note.channel,
            key: note.key,
        }
    }
}

/// The per-note controllers of the keys that have any: MIDI 2.0 keeps them
/// for the key, so a note starts from what its key's last note was left at,
/// and one may come before its note does.
#[derive(Debug)]
pub(super) struct Keys {
    entries: Box<[KeyEntry]>,
    /// Counts writes, so the least recently written entry is the one reused.
    clock: u64,
}

#[derive(Debug, Clone, Copy)]
struct KeyEntry {
    place: Option<Place>,
    controls: NoteControls,
    written: u64,
}

impl Keys {
    /// Room for `count` keys.
    pub(super) fn new(count: usize) -> Self {
        let free = KeyEntry {
            place: None,
            controls: NoteControls::NEW,
            written: 0,
        };
        Keys {
            entries: vec![free; count].into_boxed_slice(),
            clock: 0,
        }
    }

    /// What `place` has been set to, if anything.
    pub(super) fn get(&self, place: Place) -> Option<&NoteControls> {
        self.entries
            .iter()
            .find(|entry| entry.place == Some(place))
            .map(|entry| &entry.controls)
    }

    /// The controllers of `place` to write: its own entry, else a free one,
    /// else the least recently written, which is forgotten.
    pub(super) fn write(&mut self, place: Place) -> Option<&mut NoteControls> {
        self.clock = self.clock.wrapping_add(1);
        let index = match self.entries.iter().position(|e| e.place == Some(place)) {
            Some(index) => index,
            None => {
                let (index, _) = self
                    .entries
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, entry)| (entry.place.is_some(), entry.written))?;
                index
            }
        };
        let entry = self.entries.get_mut(index)?;
        if entry.place != Some(place) {
            *entry = KeyEntry {
                place: Some(place),
                controls: NoteControls::NEW,
                written: 0,
            };
        }
        entry.written = self.clock;
        Some(&mut entry.controls)
    }

    /// Forgets what `place` was set to, so its next note starts from the
    /// defaults.
    pub(super) fn clear(&mut self, place: Place) {
        for entry in self.entries.iter_mut() {
            if entry.place == Some(place) {
                entry.place = None;
            }
        }
    }

    /// The controllers of every key on `channel` of `group`.
    pub(super) fn on_channel(
        &mut self,
        group: Group,
        channel: Channel,
    ) -> impl Iterator<Item = &mut NoteControls> {
        self.entries
            .iter_mut()
            .filter(move |entry| {
                entry
                    .place
                    .is_some_and(|place| place.group == group && place.channel == channel)
            })
            .map(|entry| &mut entry.controls)
    }
}

#[cfg(test)]
mod tests {
    use super::{Keys, Place, Set, note_slot};
    use crate::{Channel, Group, Key};

    fn place(key: u8) -> Place {
        Place {
            group: Group::FIRST,
            channel: Channel::FIRST,
            key: Key::new(key),
        }
    }

    #[test]
    fn a_set_value_reads_back_and_an_unset_one_reads_none() {
        let mut set = Set::<128>::EMPTY;
        set.put(127, 0.25);
        assert_eq!((set.get(127), set.get(126)), (Some(0.25), None));
        set.clear(127);
        assert_eq!(set.get(127), None);
        // Past the end, nothing is kept and nothing panics.
        set.put(128, 1.0);
        assert_eq!(set.get(128), None);
    }

    #[test]
    fn a_note_keeps_only_the_controllers_midi_2_defines() {
        assert_eq!(note_slot(1), Some(0));
        assert_eq!(note_slot(74), Some(10));
        assert_eq!(note_slot(95), Some(20));
        assert_eq!(note_slot(3), None, "pitch has a place of its own");
        assert_eq!(note_slot(12), None);
    }

    #[test]
    fn a_full_table_forgets_the_key_least_recently_written() {
        let mut keys = Keys::new(2);
        keys.write(place(60)).expect("room").bend = crate::Centred::new(0.5);
        keys.write(place(61)).expect("room");
        keys.write(place(60)).expect("its own");
        keys.write(place(62)).expect("61's room");
        assert!(keys.get(place(61)).is_none());
        assert_eq!(keys.get(place(60)).map(|c| c.bend.get()), Some(0.5));
        assert!(keys.get(place(62)).is_some());
    }
}
