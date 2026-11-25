//! The MIDI probe, which makes a host's MIDI audible and sends it back out,
//! for checking MIDI through a real host by hand.
//!
//! Each note plays a sine at its pitch, as loud as its velocity.
//!
//! - A note's own pitch, bend, pressure and volume move it, so a VST3 host's
//!   note expressions and MIDI 2.0's per-note controllers are heard.
//!   Pressure adds the octave above.
//! - A channel's pitch bend moves its notes by its bend range, two semitones
//!   until a host sets another.
//! - A channel's volume (controller 7) sets its notes' level.
//! - All notes off and all sound off release the channel's notes.
//!
//! Every message goes back out at its frame as it came, so a host's MIDI
//! monitor, or a track that records the probe, shows what reached the plugin.
//!
//! - With the `vst3` feature, on by default, its `cdylib` is a VST3 module
//!   through Hapto's exporter.
//! - With only `cabi`, it exports the C interface that the JUCE adapter
//!   builds on.
//! - With `standalone`, it is an app of its own, which plays the computer's
//!   MIDI inputs.

#[cfg(feature = "vst3")]
hapto_host::export_vst3!(MidiProbe);
#[cfg(feature = "cabi")]
hapto_host::export_cabi!(MidiProbe);

use std::f32::consts::TAU;

use hapto_host::layout::StereoOut;
use hapto_host::midi::controller::{self, number};
use hapto_host::midi::{Channel, Group, Message, NoteId, NoteKey, NoteOn, PerChannel, Pitch};
use hapto_host::{AddSample, Block, Parts, Plugin, PluginInfo, ProcessSetup, Processor, Status};

/// How many notes sound at once. A further note takes the quietest voice.
const VOICES: usize = 16;
/// How long a note takes to reach its level, in seconds.
const ATTACK: f32 = 0.005;
/// How long a released note takes to fall 60 dB, in seconds.
const RELEASE: f32 = 0.2;
/// The level at which a released note stops, 60 dB down.
const SILENT: f32 = 1e-3;
/// A note's own pitch bend at either end, in semitones, which is MIDI 2.0's
/// default per-note bend range.
const NOTE_BEND_RANGE: f32 = 48.0;
/// The level of a note at velocity 1 and full volume.
const LEVEL: f32 = 0.25;
/// A channel's bend range, in semitones, until a host sets one.
const BEND_RANGE: f32 = 2.0;

/// The main-thread half of the plugin, which holds nothing.
#[derive(Debug)]
pub struct MidiProbe;

impl Plugin for MidiProbe {
    const INFO: PluginInfo = hapto_host::manifest_info! {
        midi_input: true,
        midi_output: true,
    };

    type Params = ();

    type Processor = Probe;

    fn new(_: (), _parts: Parts) -> Self {
        MidiProbe
    }

    fn activate(&mut self, setup: &ProcessSetup) -> Probe {
        Probe::new(setup.sample_rate as f32)
    }
}

/// The audio-thread half, which plays sixteen sines from the block's MIDI
/// and sends that MIDI back out.
#[derive(Debug)]
pub struct Probe {
    sample_rate: f32,
    /// What a held note's level gains each frame until it reaches 1.
    attack: f32,
    /// What a released note's level is multiplied by each frame.
    fade: f32,
    voices: [Voice; VOICES],
    /// What each group's channels set for their notes.
    channels: PerChannel<ChannelState>,
}

/// One sine, and the note it plays.
#[derive(Debug, Clone, Copy, Default)]
struct Voice {
    /// The note, and the group and channel that it came on, or `None` while
    /// silent.
    note: Option<(NoteId, Group, Channel)>,
    /// Whether its key is still down.
    held: bool,
    /// Its pitch, in semitones above key 0.
    pitch: f32,
    /// Its own bend, in semitones.
    bend: f32,
    /// Its own pressure, from 0 to 1.
    pressure: f32,
    /// Its own volume, from 0 to 1.
    volume: f32,
    velocity: f32,
    /// Its envelope's level, from 0 to 1.
    level: f32,
    phase: f32,
    /// How far its phase moves each frame, as of the span's start.
    step: f32,
    /// What its sine is multiplied by, as of the span's start.
    gain: f32,
}

/// What a channel's messages set for its notes.
#[derive(Debug, Clone, Copy)]
struct ChannelState {
    /// The volume (controller 7), from 0 to 1.
    volume: f32,
    /// The pitch bend, from -1 to 1.
    bend: f32,
    /// How many semitones a full bend moves.
    range: f32,
}

impl Default for ChannelState {
    fn default() -> Self {
        ChannelState {
            volume: controller::VOLUME.default(),
            bend: 0.0,
            range: BEND_RANGE,
        }
    }
}

impl Probe {
    fn new(sample_rate: f32) -> Self {
        Probe {
            sample_rate,
            attack: 1.0 / (ATTACK * sample_rate).max(1.0),
            fade: (SILENT.ln() / (RELEASE * sample_rate).max(1.0)).exp(),
            voices: [Voice::default(); VOICES],
            channels: PerChannel::default(),
        }
    }

    /// Acts on one message, at the start of the span it arrives in.
    fn apply(&mut self, message: &Message<'_>) {
        match message {
            Message::NoteOn(note) => self.start(note),
            Message::NoteOff(note) => {
                for voice in &mut self.voices {
                    if voice.note.is_some_and(|(id, ..)| id == note.id) {
                        voice.held = false;
                    }
                }
            }
            Message::NotePitch { note, pitch } => {
                self.each_on(*note, |voice| voice.pitch = pitch.semitones());
            }
            Message::NoteBend { note, value } => {
                self.each_on(*note, |voice| voice.bend = value.get() * NOTE_BEND_RANGE);
            }
            Message::NotePressure { note, value } => {
                self.each_on(*note, |voice| voice.pressure = *value);
            }
            Message::Bend {
                group,
                channel,
                value,
            } => self.channels[(*group, *channel)].bend = value.get(),
            Message::BendRange {
                group,
                channel,
                semitones,
            } => self.channels[(*group, *channel)].range = *semitones,
            Message::Control {
                group,
                channel,
                controller: number::ALL_NOTES_OFF | number::ALL_SOUND_OFF,
                ..
            } => {
                for voice in &mut self.voices {
                    if voice
                        .note
                        .is_some_and(|(_, on, at)| on == *group && at == *channel)
                    {
                        voice.held = false;
                    }
                }
            }
            _ => {}
        }
        if let Some((group, channel, volume)) = controller::VOLUME.read_channel(message) {
            self.channels[(group, channel)].volume = volume;
        }
        if let Some((note, volume)) = controller::VOLUME.read_note(message) {
            self.each_on(note, |voice| voice.volume = volume);
        }
    }

    /// Starts `note` on a silent voice, or else on the quietest.
    fn start(&mut self, note: &NoteOn) {
        let quietest = self.voices.iter_mut().min_by(|a, b| {
            (a.note.is_some().cmp(&b.note.is_some())).then(a.level.total_cmp(&b.level))
        });
        if let Some(voice) = quietest {
            *voice = Voice {
                note: Some((note.id, note.group, note.channel)),
                held: true,
                pitch: note.pitch().semitones(),
                volume: 1.0,
                velocity: note.velocity,
                ..Voice::default()
            };
        }
    }

    /// Changes the voice, if any, that plays the note that `key` names, which
    /// the decoder has already found.
    fn each_on(&mut self, key: NoteKey, change: impl FnOnce(&mut Voice)) {
        let playing = |voice: &&mut Voice| voice.note.is_some_and(|(id, ..)| Some(id) == key.id);
        if let Some(voice) = self.voices.iter_mut().find(playing) {
            change(voice);
        }
    }

    /// Sets each voice's step and gain from its note and its channel, as of
    /// the span's start.
    fn tune(&mut self) {
        for voice in &mut self.voices {
            let Some((_, group, channel)) = voice.note else {
                continue;
            };
            let channel = self.channels[(group, channel)];
            let pitch = Pitch::new(voice.pitch + voice.bend + channel.bend * channel.range);
            voice.step = pitch.hz() / self.sample_rate;
            voice.gain = LEVEL * voice.velocity * voice.volume * channel.volume;
        }
    }

    /// Returns the next frame of every voice, summed.
    fn next_sample(&mut self) -> f32 {
        let mut sum = 0.0;
        for voice in &mut self.voices {
            if voice.note.is_none() {
                continue;
            }
            if voice.held {
                voice.level = (voice.level + self.attack).min(1.0);
            } else {
                voice.level *= self.fade;
            }
            let angle = voice.phase * TAU;
            let octave = 0.5 * voice.pressure * (2.0 * angle).sin();
            sum += (angle.sin() + octave) * voice.gain * voice.level;
            voice.phase = (voice.phase + voice.step).fract();
            if !voice.held && voice.level < SILENT {
                voice.note = None;
            }
        }
        sum
    }
}

impl Processor for Probe {
    type Layout = StereoOut;

    fn process(&mut self, block: Block<'_, StereoOut>) -> Status {
        // Every message goes back out at its frame, as it came.
        let midi = block.midi;
        let (spans, mut midi_out) = block.spans_and_out();
        midi_out.extend(midi);
        for mut span in spans {
            for message in span.midi.messages() {
                self.apply(&message);
            }
            self.tune();
            for frame in span.audio.frames_mut() {
                frame.add(self.next_sample());
            }
        }
        if self.voices.iter().any(|voice| voice.note.is_some()) {
            Status::Continue
        } else {
            Status::Sleep
        }
    }
}
