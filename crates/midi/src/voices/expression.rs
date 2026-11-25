//! What a sounding note plays at, combined from every source.

use hapto_params::Float;

use super::state::{ChannelState, NoteControls};
use super::tuning::Tuning;
use crate::controller::{self, Address, Controller, ControllerValue};
use crate::message::NoteOn;
use crate::values::{Centred, Key, Pitch};

/// What a sounding note plays at now, from every source, for a
/// [`Voice`](super::Voice) to render.
///
/// [`Poly`](super::Poly) computes it for each span, so it holds until the
/// next message. A MIDI 2.0 per-note controller replaces the channel's once
/// it arrives. With MPE on, a note on a member channel also follows its zone's
/// master channel, as [`Poly::with_mpe`](super::Poly::with_mpe) describes. A
/// test starts from [`Expression::at`] and sets the public fields it needs.
///
/// # Examples
///
/// ```
/// use hapto_midi::{Expression, Key, controller};
///
/// // A4 pressed halfway, for a test of a voice.
/// let mut expression = Expression::at(Key::A4);
/// expression.pressure = 0.5;
/// assert_eq!(expression.pitch.hz(), 440.0);
/// assert_eq!(expression.get(controller::VOLUME), controller::VOLUME.default());
/// ```
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Expression<'a> {
    /// The pitch to sound.
    ///
    /// It starts at the key's pitch in [`Poly`](super::Poly)'s [`Tuning`],
    /// or at the pitch that its note-on or a per-note pitch message set, and
    /// adds:
    ///
    /// - the note's own bend times its channel's per-note bend range
    /// - the channel's bend times its bend range
    /// - the channel's fine and coarse tuning
    /// - with MPE on, the master channel's bend times its bend range
    pub pitch: Pitch,
    /// The note's own pressure from 0 to 1, or its channel's until the note
    /// has one.
    pub pressure: f32,
    /// The note's own brightness (74), or its channel's until the note has
    /// one.
    pub timbre: Centred,
    /// The damper, from 0 while the key or a pedal holds it off the string
    /// to 1 when it rests on the string, with half pedal between. A voice
    /// that damps by it supports half pedal and a pedal pressed again.
    pub damper: f32,
    /// The note's own pan (10), from -1 left to 1 right, or its channel's
    /// until the note has one. [`VoiceAudio`](super::VoiceAudio) already
    /// places a mono voice by it.
    pub pan: Centred,
    sources: Option<Sources<'a>>,
}

/// A note's controller sources: its own, its channel's and its MPE zone
/// master's.
#[derive(Debug, Clone, Copy)]
struct Sources<'a> {
    own: &'a NoteControls,
    channel: &'a ChannelState,
    master: Option<&'a ChannelState>,
}

impl Sources<'_> {
    /// Returns the value of control change `number` from the first source
    /// that has one.
    fn control(&self, number: u8) -> Option<f32> {
        let address = Address::Control(number);
        self.own
            .control(number)
            .or_else(|| self.channel.control(address))
            .or_else(|| self.master?.control(address))
    }
}

impl Expression<'static> {
    /// Returns a note at `key` in equal temperament, with its key held and
    /// everything at rest, for testing a voice on its own.
    pub fn at(key: Key) -> Self {
        Expression {
            pitch: Pitch::from(key),
            pressure: 0.0,
            timbre: Centred::CENTRE,
            damper: 0.0,
            pan: Centred::CENTRE,
            sources: None,
        }
    }
}

impl<'a> Expression<'a> {
    /// Returns what `note` plays at, from its own controllers, its channel's
    /// and its zone master's.
    pub(super) fn of(
        note: &NoteOn,
        own: &'a NoteControls,
        (channel, master): (&'a ChannelState, Option<&'a ChannelState>),
        tuning: &Tuning,
        damper: f32,
    ) -> Self {
        let start = match note.attribute.semitones() {
            Some(semitones) => semitones as f32,
            None => tuning.pitch(note.key).semitones(),
        };
        let base = own.pitch.map_or(start, Pitch::semitones);
        let pitch = base
            + own.bend.get() * channel.note_bend_range
            + channel.bend.get() * channel.bend_range
            + channel.fine
            + channel.coarse
            + master.map_or(0.0, |master| master.bend.get() * master.bend_range);
        let sources = Sources {
            own,
            channel,
            master,
        };
        let centred = |number: u8| {
            sources
                .control(number)
                .map_or(Centred::CENTRE, Centred::from_unit)
        };
        Expression {
            pitch: Pitch::new(pitch),
            pressure: own.pressure.unwrap_or(channel.pressure),
            timbre: centred(controller::number::BRIGHTNESS),
            damper,
            pan: centred(controller::number::PAN),
            sources: Some(sources),
        }
    }

    /// Returns the note's value of `controller`, which is its own, or else
    /// its channel's, or else its MPE zone master's, or else the controller's
    /// default.
    ///
    /// Open: only control changes are kept, so a registered or assignable
    /// controller reads as its default. The tunings and bend ranges among
    /// them are already in [`pitch`](Self::pitch).
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_midi::controller::{self, Controller};
    /// use hapto_midi::{Expression, Key};
    ///
    /// // A plugin's own controller (20), which rests at 0.7.
    /// const DRIVE: Controller<f32> = Controller::control(20, 0.7);
    /// let expression = Expression::at(Key::A4);
    /// assert_eq!(expression.get(DRIVE), 0.7);
    /// assert!(!expression.get(controller::SUSTAIN).is_down());
    /// ```
    pub fn get<T: ControllerValue>(&self, controller: Controller<T>) -> T {
        let value = match (self.sources, controller.address()) {
            (Some(sources), Address::Control(number)) => sources.control(number),
            _ => None,
        };
        value.map_or(controller.default(), T::from_unit)
    }

    /// Returns `param`'s own value plus any polyphonic modulation that the
    /// host sends this note.
    ///
    /// Open: polyphonic modulation comes with the CLAP exporter. Until then
    /// this returns the parameter's own value, so a voice that calls it now
    /// needs no change later.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_midi::{Expression, Key};
    /// use hapto_params::{Float, Kind, ParamDecl, ParamSet};
    ///
    /// let set = ParamSet::from_decls([
    ///     ParamDecl::new("cutoff", "Cutoff", Kind::continuous(20.0, 20_000.0, 1.0, None), 800.0),
    /// ])?;
    /// let cutoff: Float = set.bind("cutoff")?;
    /// assert_eq!(Expression::at(Key::A4).modulated(&cutoff), 800.0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn modulated(&self, param: &Float) -> f32 {
        param.get()
    }
}
