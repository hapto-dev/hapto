//! What a sounding note plays at, from every source, combined.

use hapto_params::Float;

use super::state::{ChannelState, NoteControls};
use super::tuning::Tuning;
use crate::controller::{self, Address, Controller, ControllerValue};
use crate::message::NoteOn;
use crate::values::{Centred, Key, Pitch};

/// What a sounding note plays at now, from every source, combined: what a
/// [`Voice`](super::Voice) renders by. [`Poly`](super::Poly) works it out
/// for each span, so it holds until the next message.
///
/// A note's own controller, MIDI 2.0's per-note one, takes the place of its
/// channel's once it has arrived. With MPE on, a note on a zone's member
/// channel answers its zone's master too: see
/// [`Poly::with_mpe`](super::Poly::with_mpe). Its fields are public, so a
/// test sets what it needs on [`Expression::at`].
///
/// # Examples
///
/// ```
/// use hapto_midi::{Expression, Key, controller};
///
/// // A voice's test: A4, pressed halfway.
/// let mut expression = Expression::at(Key::A4);
/// expression.pressure = 0.5;
/// assert_eq!(expression.pitch.hz(), 440.0);
/// assert_eq!(expression.get(controller::VOLUME), controller::VOLUME.default());
/// ```
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Expression<'a> {
    /// The pitch to sound: its key's in [`Poly`](super::Poly)'s
    /// [`Tuning`], or the one its note-on or a message of its own gave it,
    /// moved by its own bend times its channel's per-note bend range, its
    /// channel's bend times the bend range, its channel's fine and coarse
    /// tuning, and with MPE on its zone's master's bend times that one's
    /// range.
    pub pitch: Pitch,
    /// From 0 to 1: the note's own pressure once it has had one, its
    /// channel's until then.
    pub pressure: f32,
    /// The note's own brightness, controller 74, once it has had one, its
    /// channel's until then.
    pub timbre: Centred,
    /// From 0, while the key or a pedal holds the damper off the string, to
    /// 1, with it on the string, and between at half pedal: a voice that
    /// damps by it gets half pedal and a pedal pressed again.
    pub damper: f32,
    /// Where the note sits, -1 left to 1 right: its own pan, controller 10,
    /// once it has had one, its channel's until then. A mono voice is placed
    /// by it already: see [`VoiceAudio`](super::VoiceAudio).
    pub pan: Centred,
    sources: Option<Sources<'a>>,
}

/// Where a note's controllers come from: its own, its channel's, and its
/// MPE zone's master's.
#[derive(Debug, Clone, Copy)]
struct Sources<'a> {
    own: &'a NoteControls,
    channel: &'a ChannelState,
    master: Option<&'a ChannelState>,
}

impl Sources<'_> {
    /// The value of control change `number`, from the first source that
    /// has one.
    fn control(&self, number: u8) -> Option<f32> {
        let address = Address::Control(number);
        self.own
            .control(number)
            .or_else(|| self.channel.control(address))
            .or_else(|| self.master?.control(address))
    }
}

impl Expression<'static> {
    /// A note at `key` in equal temperament, its key held and everything at
    /// rest: for testing a voice on its own.
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
    /// What `note` plays at, from its own controllers, its channel's, and
    /// its zone's master's.
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

    /// The note's value of `controller`: its own where it has one, its
    /// channel's otherwise, its MPE zone's master's after that, and the
    /// controller's default before any.
    ///
    /// Open: only control changes are kept. A registered or assignable
    /// controller reads as its default; the tunings and bend ranges among
    /// them are in [`pitch`](Self::pitch) already.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_midi::controller::{self, Controller};
    /// use hapto_midi::{Expression, Key};
    ///
    /// // A plugin's own controller, 20, resting at 0.7.
    /// const DRIVE: Controller<f32> = Controller::control(20, 0.7);
    /// let expression = Expression::at(Key::C4);
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

    /// `param`'s value for this note: the parameter's own, plus whatever
    /// polyphonic modulation its host sends this note.
    ///
    /// Open: polyphonic modulation comes with the CLAP exporter. Until then
    /// this is the parameter's own value, so a voice written now needs no
    /// change when it comes.
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
    /// assert_eq!(Expression::at(Key::C4).modulated(&cutoff), 800.0);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn modulated(&self, param: &Float) -> f32 {
        param.get()
    }
}
