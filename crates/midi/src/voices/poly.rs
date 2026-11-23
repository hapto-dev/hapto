//! Voices for notes: which voice plays which note, and what holds it.

use std::marker::PhantomData;

use hapto_audio::layout::Layout;
use hapto_audio::{Audio, Status};

use super::Voice;
use super::audio::VoiceAudio;
use super::expression::Expression;
use super::mpe::{Mpe, Zones, master_of};
use super::state::{ChannelState, Channels, Keys, NoteControls, Place, note_slot};
use super::tuning::Tuning;
use crate::controller::{self, ControllerValue, number};
use crate::message::{Message, NoteKey, NoteOff, NoteOn};
use crate::values::{Centred, Channel, Group, NoteId, Pedal};
use crate::{Midi, Spans};

/// How many keys' per-note controllers a [`Poly`] keeps at once.
const KEYS: usize = 256;

/// Voices for notes, as many at once as it was made with.
///
/// - **Starting.** A note-on takes an idle voice; with none idle it takes a
///   released one, then one only a pedal holds, then one whose key is held,
///   the oldest of each first. A voice taken from a note is started again
///   with no word to it first.
/// - **The same key twice.** A second note-on on a key that sounds starts a
///   second note, and the first keeps sounding: MIDI 2.0 sends a second
///   note on a key, and a merged track may. A note only a pedal holds is
///   released when its key is struck again, so repeated notes under the
///   pedal do not pile up.
/// - **Per-note controllers** belong to the key in MIDI 2.0: a note starts
///   from what its key's last note was left at, until per-note management
///   resets the key, and one may come before its note. A note its host
///   names, as VST3's are, starts from the defaults and keeps its own. The
///   keys of the last 256 written are kept.
/// - **Pedals.** The sustain pedal is continuous: a voice is released when
///   nothing holds its note, and [`Expression::damper`] follows half pedal.
///   The sostenuto pedal holds the notes whose keys are down as it goes
///   down.
/// - **Resets and modes.** Controllers 120 to 127 as MIDI 1.0 defines them:
///   All Sound Off frees the channel's voices at once, Reset All
///   Controllers resets the bend, the pressures, the modulation wheel,
///   expression and the pedals, and All Notes Off and the mode messages
///   release the notes whose keys are held.
/// - **Groups.** Each group is 16 channels of its own, and `Poly` plays all.
/// - **Tuning.** Every note's pitch starts from its [`Tuning`], equal
///   temperament unless the plugin sets one.
/// - **MPE** is off unless the plugin turns it on with
///   [`with_mpe`](Self::with_mpe).
///
/// It works at two levels, as a block does. [`play`](Self::play) takes a
/// whole block. Inside a loop over [`Spans`], [`handle`](Self::handle)
/// takes the span's messages and [`render`](Self::render) its audio, so a
/// plugin can process the voices' sound, or route messages between several.
///
/// # Examples
///
/// ```
/// use hapto_audio::AudioBuf;
/// use hapto_audio::layout::StereoOut;
/// use hapto_midi::{Expression, Key, Message, NoteOff, NoteOn, Poly, Voice, VoiceAudio};
///
/// /// A sine at the note's pitch, cut off at its release.
/// #[derive(Default)]
/// struct Sine {
///     phase: f32,
///     on: bool,
/// }
///
/// impl Voice<StereoOut> for Sine {
///     fn start(&mut self, _: &NoteOn) {
///         (self.phase, self.on) = (0.0, true);
///     }
///
///     fn release(&mut self, _: &NoteOff) {
///         self.on = false;
///     }
///
///     fn render(&mut self, expression: &Expression<'_>, out: &mut VoiceAudio<'_, StereoOut>) {
///         if !self.on {
///             return;
///         }
///         let step = expression.pitch.hz() / 48_000.0;
///         for frame in out.frames_mut() {
///             frame.add(0.1 * (self.phase * std::f32::consts::TAU).sin());
///             self.phase = (self.phase + step).fract();
///         }
///     }
///
///     fn is_done(&self) -> bool {
///         !self.on
///     }
/// }
///
/// let mut voices: Poly<Sine, StereoOut> = Poly::new(8, Sine::default);
/// voices.handle(&Message::NoteOn(NoteOn::new(Key::A4, 1.0)));
/// let mut buffer = AudioBuf::<StereoOut>::new(64);
/// voices.render(&mut buffer.audio());
/// assert!(buffer.audio().peak() > 0.0);
/// assert_eq!(voices.notes().count(), 1);
/// ```
#[derive(Debug)]
pub struct Poly<V, L> {
    slots: Box<[Slot<V>]>,
    channels: Channels,
    keys: Keys,
    tuning: Tuning,
    /// Each group's MPE zones; `None` while MPE is off.
    mpe: Option<Mpe>,
    /// Counts note-ons, so the oldest note is the one stolen.
    clock: u64,
    layout: PhantomData<fn() -> L>,
}

/// A voice, and the note it plays.
#[derive(Debug)]
struct Slot<V> {
    voice: V,
    /// `None` while idle.
    note: Option<Sounding>,
}

#[derive(Debug, Clone, Copy)]
struct Sounding {
    on: NoteOn,
    hold: Hold,
    /// Caught by the sostenuto pedal as it went down, so held until it
    /// comes up.
    sostenuto: bool,
    started: u64,
    own: NoteControls,
}

/// What still holds a note.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Hold {
    /// Its key is down.
    Key,
    /// Its key is up, and a pedal holds it: the note-off, kept for its
    /// release.
    Pedal(NoteOff),
    /// Nothing: its voice has been released.
    Released,
}

impl Sounding {
    fn is_on(&self, group: Group, channel: Channel) -> bool {
        self.on.group == group && self.on.channel == channel
    }

    /// Which goes first when a voice must be taken: lowest first.
    fn rank(&self) -> (u8, u64) {
        let hold = match self.hold {
            Hold::Released => 1,
            Hold::Pedal(_) => 2,
            Hold::Key => 3,
        };
        (hold, self.started)
    }

    fn damper(&self, sustain: Pedal) -> f32 {
        match self.hold {
            Hold::Key => 0.0,
            _ if self.sostenuto => 0.0,
            _ => 1.0 - sustain.depth(),
        }
    }

    /// Whether a pedal holds the note once its key is up, `sustain` being
    /// the sustain pedal it answers to.
    fn pedal_holds(&self, sustain: Pedal) -> bool {
        self.sostenuto || sustain.is_down()
    }
}

/// The channels a note answers to: its own, and its zone's master's when it
/// is a member of an MPE zone.
fn sources<'c>(
    channels: &'c Channels,
    mpe: Option<&Mpe>,
    on: &NoteOn,
) -> (&'c ChannelState, Option<&'c ChannelState>) {
    let master = master_of(mpe, on.group, on.channel);
    (
        channels.of(on.group, on.channel),
        master.map(|master| channels.of(on.group, master)),
    )
}

/// The sustain pedal a note answers to: its channel's, or its zone's
/// master's, whichever is further down.
fn sustain(channels: &Channels, mpe: Option<&Mpe>, on: &NoteOn) -> Pedal {
    let (own, master) = sources(channels, mpe, on);
    let own = own.read(controller::SUSTAIN);
    match master.map(|master| master.read(controller::SUSTAIN)) {
        Some(zone) if zone.depth() > own.depth() => zone,
        _ => own,
    }
}

/// Whether a message on `channel` of `group` reaches `note`: one on its own
/// channel does, and with MPE on, so does one on its zone's master.
fn reaches(mpe: Option<&Mpe>, group: Group, channel: Channel, note: &Sounding) -> bool {
    note.on.group == group
        && (note.on.channel == channel
            || master_of(mpe, note.on.group, note.on.channel) == Some(channel))
}

impl<V, L> Poly<V, L> {
    /// `count` voices, each made by `voice`, all idle.
    ///
    /// Thread: any but audio. It allocates.
    pub fn new(count: usize, mut voice: impl FnMut() -> V) -> Self {
        Poly {
            slots: (0..count)
                .map(|_| Slot {
                    voice: voice(),
                    note: None,
                })
                .collect(),
            channels: Channels::new(),
            keys: Keys::new(KEYS),
            tuning: Tuning::equal(),
            mpe: None,
            clock: 0,
            layout: PhantomData,
        }
    }

    /// The same voices, playing in `tuning`.
    #[must_use]
    pub fn with_tuning(mut self, tuning: Tuning) -> Self {
        self.tuning = tuning;
        self
    }

    /// Sets the tuning every note's pitch starts from; notes sounding move
    /// to it at once.
    pub fn set_tuning(&mut self, tuning: Tuning) {
        self.tuning = tuning;
    }

    /// The same voices, playing MPE: each group starts with MPE's default
    /// layout, a lower zone of 15 member channels under channel 1, and an
    /// MPE Configuration Message changes a group's zones.
    ///
    /// A member channel's bend range starts at MPE's 48 semitones, and its
    /// master's at 2; a configuration message returns a zone's to them. A
    /// note on a member channel takes its bend, pressure and controllers
    /// from its own channel, as MPE sends them for each note, adds its
    /// master's bend to its pitch, answers its master's pedals and its
    /// notes-off and sound-off messages, and reads a controller its own
    /// channel has not set from its master's.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_audio::AudioBuf;
    /// use hapto_audio::layout::Mono;
    /// use hapto_midi::{Centred, Channel, Expression, Group, Key, Message, NoteOff, NoteOn};
    /// use hapto_midi::{Poly, Voice, VoiceAudio};
    ///
    /// #[derive(Default)]
    /// struct Heard {
    ///     pitch: f32,
    /// }
    ///
    /// impl Voice<Mono> for Heard {
    ///     fn start(&mut self, _: &NoteOn) {}
    ///     fn release(&mut self, _: &NoteOff) {}
    ///     fn render(&mut self, expression: &Expression<'_>, _: &mut VoiceAudio<'_, Mono>) {
    ///         self.pitch = expression.pitch.semitones();
    ///     }
    ///     fn is_done(&self) -> bool {
    ///         false
    ///     }
    /// }
    ///
    /// let mut voices: Poly<Heard, Mono> = Poly::new(15, Heard::default).with_mpe();
    /// let bend = |channel, value| Message::Bend {
    ///     group: Group::FIRST,
    ///     channel: Channel::number(channel),
    ///     value: Centred::new(value),
    /// };
    /// // A note on member channel 2, bent a quarter of its 48 semitones up,
    /// // and the whole zone half of its master's 2 down.
    /// let mut note = NoteOn::new(Key::C4, 1.0);
    /// note.channel = Channel::number(2);
    /// voices.handle(&Message::NoteOn(note));
    /// voices.handle(&bend(2, 0.25));
    /// voices.handle(&bend(1, -0.5));
    /// voices.render(&mut AudioBuf::<Mono>::new(1).audio());
    /// let pitch = voices.voices_mut().map(|voice| voice.pitch).fold(0.0, f32::max);
    /// assert_eq!(pitch, 60.0 + 12.0 - 1.0);
    /// ```
    #[must_use]
    pub fn with_mpe(mut self) -> Self {
        let mut mpe = [Zones::LOWER_15; 16];
        for (group, zones) in (0..16).map(Group::from_wire).zip(&mut mpe) {
            configure(&mut self.channels, group, zones, Channel::FIRST, 15);
        }
        self.mpe = Some(mpe);
        self
    }

    /// Every voice, idle or not, in order: to set what a parameter changes
    /// on each.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_audio::layout::Mono;
    /// use hapto_midi::{Expression, NoteOff, NoteOn, Poly, Voice, VoiceAudio};
    ///
    /// #[derive(Default)]
    /// struct Pluck {
    ///     decay: f32,
    /// }
    /// # impl Voice<Mono> for Pluck {
    /// #     fn start(&mut self, _: &NoteOn) {}
    /// #     fn release(&mut self, _: &NoteOff) {}
    /// #     fn render(&mut self, _: &Expression<'_>, _: &mut VoiceAudio<'_, Mono>) {}
    /// #     fn is_done(&self) -> bool { true }
    /// # }
    ///
    /// let mut voices: Poly<Pluck, Mono> = Poly::new(4, Pluck::default);
    /// // The decay parameter moved: every voice takes it.
    /// voices.voices_mut().for_each(|voice| voice.decay = 0.5);
    /// ```
    pub fn voices_mut(&mut self) -> impl Iterator<Item = &mut V> {
        self.slots.iter_mut().map(|slot| &mut slot.voice)
    }

    /// The notes sounding, each as its note-on came: what a page's keyboard
    /// lights.
    pub fn notes(&self) -> impl Iterator<Item = &NoteOn> {
        self.slots
            .iter()
            .filter_map(|slot| slot.note.as_ref().map(|note| &note.on))
    }

    /// Whether no voice sounds.
    pub fn is_silent(&self) -> bool {
        self.slots.iter().all(|slot| slot.note.is_none())
    }
}

/// Sets the zone of `group` whose master is `channel` to `members` member
/// channels, and returns its channels' bend ranges to MPE's.
fn configure(
    channels: &mut Channels,
    group: Group,
    zones: &mut Zones,
    channel: Channel,
    members: u8,
) {
    if let Some(members) = zones.configure(channel, members) {
        channels.of_mut(group, channel).bend_range = 2.0;
        for member in members {
            channels.of_mut(group, member).bend_range = 48.0;
        }
    }
}

impl<V: Voice<L>, L: Layout> Poly<V, L> {
    /// Plays a block: splits it at each message, acts on each, and renders
    /// the voices between, into its audio. [`Status::Sleep`] once no voice
    /// sounds.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_audio::layout::StereoOut;
    /// use hapto_midi::{Expression, NoteOff, NoteOn, Poly, Voice, VoiceAudio};
    /// # use hapto_audio::{Audio, Status};
    /// # use hapto_midi::Midi;
    ///
    /// /// The plugin's audio-thread half.
    /// struct Synth {
    ///     voices: Poly<Click, StereoOut>,
    /// }
    ///
    /// impl Synth {
    ///     // What `Processor::process` does with its block:
    ///     // `self.voices.play(block)`.
    ///     fn process(&mut self, audio: Audio<'_, StereoOut>, midi: Midi<'_>) -> Status {
    ///         self.voices.play((audio, midi))
    ///     }
    /// }
    /// # struct Click;
    /// # impl Voice<StereoOut> for Click {
    /// #     fn start(&mut self, _: &NoteOn) {}
    /// #     fn release(&mut self, _: &NoteOff) {}
    /// #     fn render(&mut self, _: &Expression<'_>, _: &mut VoiceAudio<'_, StereoOut>) {}
    /// #     fn is_done(&self) -> bool { true }
    /// # }
    /// # let mut buffer = hapto_audio::AudioBuf::<StereoOut>::new(8);
    /// # let mut synth = Synth { voices: Poly::new(1, || Click) };
    /// # assert_eq!(synth.process(buffer.audio(), Midi::default()), Status::Sleep);
    /// ```
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn play<'a>(&mut self, block: impl Into<ToPlay<'a, L>>) -> Status {
        let ToPlay { audio, midi } = block.into();
        for mut span in Spans::new(audio, midi) {
            for message in span.midi.messages() {
                self.handle(&message);
            }
            self.render(&mut span.audio);
        }
        if self.is_silent() {
            Status::Sleep
        } else {
            Status::Continue
        }
    }

    /// Acts on a message: starts, releases or takes a voice, or changes what
    /// voices read from their [`Expression`]. Other messages pass.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn handle(&mut self, message: &Message<'_>) {
        match *message {
            Message::NoteOn(note) => self.start(note),
            Message::NoteOff(off) => self.end(off),
            Message::NotePitch { note, pitch } => {
                self.each_note(note, |own| own.pitch = Some(pitch))
            }
            Message::NoteBend { note, value } => self.each_note(note, |own| own.bend = value),
            Message::NotePressure { note, value } => {
                self.each_note(note, |own| own.pressure = Some(value));
            }
            Message::NoteControl {
                note,
                controller,
                value,
            } => {
                if let Some(slot) = note_slot(controller) {
                    self.each_note(note, |own| own.controls.put(slot, value));
                }
            }
            Message::NoteManagement {
                note,
                detach,
                reset,
            } => {
                if reset {
                    self.reset_key(note, detach);
                }
            }
            Message::Bend {
                group,
                channel,
                value,
            } => self.channels.of_mut(group, channel).bend = value,
            Message::BendRange {
                group,
                channel,
                semitones,
            } => self.channels.of_mut(group, channel).bend_range = semitones,
            Message::Pressure {
                group,
                channel,
                value,
            } => self.channels.of_mut(group, channel).pressure = value,
            Message::Registered {
                group,
                channel,
                bank: 0,
                index,
                value,
            } => self.registered(group, channel, index, value),
            Message::Control {
                group,
                channel,
                controller,
                value,
            } => self.control(group, channel, controller, value),
            _ => {}
        }
    }

    /// Every sounding voice's next `out.frames()` frames, added into `out`,
    /// and the voices that are done freed.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn render(&mut self, out: &mut Audio<'_, L>) {
        for slot in self.slots.iter_mut() {
            let Some(note) = &slot.note else {
                continue;
            };
            let mpe = self.mpe.as_ref();
            let (channel, master) = sources(&self.channels, mpe, &note.on);
            let damper = note.damper(sustain(&self.channels, mpe, &note.on));
            let expression =
                Expression::of(&note.on, &note.own, (channel, master), &self.tuning, damper);
            let mut audio = VoiceAudio::new(out.reborrow(), expression.pan);
            slot.voice.render(&expression, &mut audio);
            if slot.voice.is_done() {
                slot.note = None;
            }
        }
    }

    fn start(&mut self, on: NoteOn) {
        let place = Place {
            group: on.group,
            channel: on.channel,
            key: on.key,
        };
        // A key struck again lets go of a note on it that only a pedal holds.
        for slot in self.slots.iter_mut() {
            if let Some(note) = &mut slot.note
                && let Hold::Pedal(off) = note.hold
                && note.on.key == on.key
                && note.is_on(on.group, on.channel)
            {
                note.hold = Hold::Released;
                slot.voice.release(&off);
            }
        }
        let own = match on.id.is_hosts() {
            true => NoteControls::NEW,
            false => self.keys.get(place).copied().unwrap_or(NoteControls::NEW),
        };
        let rank = |slot: &&mut Slot<V>| slot.note.as_ref().map_or((0, 0), Sounding::rank);
        let Some(slot) = self.slots.iter_mut().min_by_key(rank) else {
            return;
        };
        self.clock = self.clock.wrapping_add(1);
        slot.note = Some(Sounding {
            on,
            hold: Hold::Key,
            sostenuto: false,
            started: self.clock,
            own,
        });
        slot.voice.start(&on);
    }

    fn end(&mut self, off: NoteOff) {
        for slot in self.slots.iter_mut() {
            if let Some(note) = &mut slot.note
                && note.hold == Hold::Key
                && note.on.id == off.id
                && note.is_on(off.group, off.channel)
            {
                if note.pedal_holds(sustain(&self.channels, self.mpe.as_ref(), &note.on)) {
                    note.hold = Hold::Pedal(off);
                } else {
                    note.hold = Hold::Released;
                    slot.voice.release(&off);
                }
            }
        }
    }

    /// Changes the controllers of the note `key` names, and of its key, as
    /// MIDI 2.0 keeps them; a note its host names keeps its own alone.
    fn each_note(&mut self, key: NoteKey, change: impl Fn(&mut NoteControls)) {
        let named = key.id.is_some_and(NoteId::is_hosts);
        if let Some(id) = key.id {
            let sounding = self
                .slots
                .iter_mut()
                .filter_map(|slot| slot.note.as_mut())
                .find(|note| note.on.id == id && note.is_on(key.group, key.channel));
            if let Some(note) = sounding {
                change(&mut note.own);
            }
        }
        if !named && let Some(own) = self.keys.write(Place::from(key)) {
            change(own);
        }
    }

    /// Returns the key's per-note controllers to their defaults: for its
    /// next note, and for the one sounding unless it was just detached.
    fn reset_key(&mut self, key: NoteKey, detach: bool) {
        self.keys.clear(Place::from(key));
        if detach {
            return;
        }
        for note in self.slots.iter_mut().filter_map(|slot| slot.note.as_mut()) {
            if Some(note.on.id) == key.id && note.is_on(key.group, key.channel) {
                note.own = NoteControls::NEW;
            }
        }
    }

    fn registered(&mut self, group: Group, channel: Channel, index: u8, value: f32) {
        // The MPE Configuration Message: its count of member channels is the
        // data's top 7 bits.
        if index == 6
            && let Some(zones) = self.mpe.as_mut().and_then(|mpe| mpe.get_mut(group.index()))
        {
            let members = (value * 128.0).floor().clamp(0.0, 15.0) as u8;
            configure(&mut self.channels, group, zones, channel, members);
            return;
        }
        let state = self.channels.of_mut(group, channel);
        match index {
            // A semitone each way.
            1 => state.fine = Centred::from_unit(value).get(),
            // Whole semitones, 64 each way.
            2 => state.coarse = (Centred::from_unit(value).get() * 64.0).round(),
            // Semitones in 7.25 fixed point: the top 7 bits are whole ones.
            7 => state.note_bend_range = value * 128.0,
            _ => {}
        }
    }

    fn control(&mut self, group: Group, channel: Channel, controller: u8, value: f32) {
        let state = self.channels.of_mut(group, channel);
        let held = state.read(controller::SOSTENUTO).is_down();
        state.set_control(controller, value);
        let holds = state.read(controller::SOSTENUTO).is_down();
        match controller {
            number::SUSTAIN => self.pedal_up(group, channel),
            number::SOSTENUTO if holds && !held => self.catch(group, channel),
            number::SOSTENUTO if held && !holds => {
                self.let_go(group, channel);
                self.pedal_up(group, channel);
            }
            number::ALL_SOUND_OFF => {
                let mpe = self.mpe.as_ref();
                for slot in self.slots.iter_mut() {
                    if slot
                        .note
                        .is_some_and(|note| reaches(mpe, group, channel, &note))
                    {
                        slot.note = None;
                    }
                }
            }
            number::RESET_ALL_CONTROLLERS => self.reset_controllers(group, channel),
            // All Notes Off, and the mode messages, which end notes too.
            number::ALL_NOTES_OFF..=127 => self.all_notes_off(group, channel),
            _ => {}
        }
    }

    /// Releases the notes on the channel whose keys are up and that no
    /// pedal holds any more.
    fn pedal_up(&mut self, group: Group, channel: Channel) {
        let mpe = self.mpe.as_ref();
        for slot in self.slots.iter_mut() {
            if let Some(note) = &mut slot.note
                && let Hold::Pedal(off) = note.hold
                && reaches(mpe, group, channel, note)
                && !note.pedal_holds(sustain(&self.channels, mpe, &note.on))
            {
                note.hold = Hold::Released;
                slot.voice.release(&off);
            }
        }
    }

    /// The sostenuto pedal went down: it holds the notes whose keys are.
    fn catch(&mut self, group: Group, channel: Channel) {
        let mpe = self.mpe.as_ref();
        for note in self.slots.iter_mut().filter_map(|slot| slot.note.as_mut()) {
            if note.hold == Hold::Key && reaches(mpe, group, channel, note) {
                note.sostenuto = true;
            }
        }
    }

    /// The sostenuto pedal came up: it holds nothing.
    fn let_go(&mut self, group: Group, channel: Channel) {
        let mpe = self.mpe.as_ref();
        for note in self.slots.iter_mut().filter_map(|slot| slot.note.as_mut()) {
            if reaches(mpe, group, channel, note) {
                note.sostenuto = false;
            }
        }
    }

    /// Ends every note on the channel whose key is held, as its note-off
    /// would, at MIDI's default note-off velocity; a pedal still holds what
    /// it holds.
    fn all_notes_off(&mut self, group: Group, channel: Channel) {
        let mpe = self.mpe.as_ref();
        for slot in self.slots.iter_mut() {
            if let Some(note) = &mut slot.note
                && note.hold == Hold::Key
                && reaches(mpe, group, channel, note)
            {
                let off = note.on.off(0.5);
                if note.pedal_holds(sustain(&self.channels, mpe, &note.on)) {
                    note.hold = Hold::Pedal(off);
                } else {
                    note.hold = Hold::Released;
                    slot.voice.release(&off);
                }
            }
        }
    }

    /// Reset All Controllers, as MIDI 1.0's recommended practice has it:
    /// the bend, the channel's and its notes' pressure, the modulation
    /// wheel, expression and the four pedals return to their defaults.
    /// Volume, pan, the sound controllers, the ranges and the tunings stay,
    /// and so do MIDI 2.0's per-note controllers but pressure.
    fn reset_controllers(&mut self, group: Group, channel: Channel) {
        let state = self.channels.of_mut(group, channel);
        state.bend = Centred::CENTRE;
        state.pressure = 0.0;
        for number in [
            number::MOD_WHEEL,
            number::EXPRESSION,
            number::SUSTAIN,
            number::PORTAMENTO,
            number::SOSTENUTO,
            number::SOFT,
        ] {
            state.controls.clear(usize::from(number));
        }
        for own in self.keys.on_channel(group, channel) {
            own.pressure = None;
        }
        for note in self.slots.iter_mut().filter_map(|slot| slot.note.as_mut()) {
            if note.is_on(group, channel) {
                note.own.pressure = None;
            }
        }
        self.let_go(group, channel);
        self.pedal_up(group, channel);
    }
}

/// What [`Poly::play`] plays: a block's audio and its MIDI. A processor's
/// block becomes one, and so do the two as a pair.
///
/// # Examples
///
/// ```
/// use hapto_audio::AudioBuf;
/// use hapto_audio::layout::Mono;
/// use hapto_midi::{Midi, ToPlay};
///
/// let mut buffer = AudioBuf::<Mono>::new(16);
/// let to_play: ToPlay<'_, Mono> = (buffer.audio(), Midi::default()).into();
/// assert_eq!(to_play.audio.frames(), 16);
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct ToPlay<'a, L: Layout> {
    /// The audio the voices are added into.
    pub audio: Audio<'a, L>,
    /// The messages to act on, each at its frame.
    pub midi: Midi<'a>,
}

impl<'a, L: Layout> ToPlay<'a, L> {
    /// `audio` with `midi`.
    pub fn new(audio: Audio<'a, L>, midi: Midi<'a>) -> Self {
        ToPlay { audio, midi }
    }
}

impl<'a, L: Layout> From<(Audio<'a, L>, Midi<'a>)> for ToPlay<'a, L> {
    fn from((audio, midi): (Audio<'a, L>, Midi<'a>)) -> Self {
        ToPlay { audio, midi }
    }
}
