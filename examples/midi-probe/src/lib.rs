//! The MIDI probe, which makes a host's MIDI audible and sends it back out,
//! for checking MIDI through a real host by hand.
//!
//! Each note plays a sine at its pitch, as loud as its velocity and its
//! volume, controller 7: its own where MIDI 2.0 or a VST3 host sets one, its
//! channel's otherwise. Its pressure adds the octave above, and its pan
//! places it. Every bend reaches its pitch: the note's own, MIDI 2.0's
//! per-note pitch, a VST3 host's tuning expression, and the channel's pitch
//! bend times the channel's bend range, two semitones until a host says
//! otherwise. The sustain pedal holds released notes; all notes off and all
//! sound off end the channel's. Every message that arrives goes back out at
//! its frame, as it came, so a host's MIDI monitor, or a track that records
//! the probe, shows what reached the plugin.
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
use hapto_host::midi::controller::VOLUME;
use hapto_host::midi::{Expression, NoteOff, NoteOn, Poly, Voice, VoiceAudio};
use hapto_host::{Block, Parts, Plugin, PluginInfo, ProcessSetup, Processor, Status};

/// How many notes sound at once.
const VOICES: usize = 16;
/// How long a note takes to reach its level, in seconds.
const ATTACK: f32 = 0.005;
/// How long a released note takes to fall 60 dB, in seconds.
const RELEASE: f32 = 0.2;
/// The level at which a released note stops, 60 dB down.
const SILENT: f32 = 1e-3;
/// The level of a note at velocity 1 and full volume.
const LEVEL: f32 = 0.25;

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
        let sample_rate = setup.sample_rate as f32;
        Probe {
            voices: Poly::new(VOICES, || Tone::new(sample_rate)),
        }
    }
}

/// The audio-thread half, which plays sixteen sines from the block's MIDI
/// and sends that MIDI back out.
#[derive(Debug)]
pub struct Probe {
    voices: Poly<Tone, StereoOut>,
}

impl Processor for Probe {
    type Layout = StereoOut;

    fn process(&mut self, block: Block<'_, StereoOut>) -> Status {
        // Every message goes back out at its frame, as it came.
        let mut midi_out = block.midi_out;
        midi_out.extend(block.midi);
        self.voices.play((block.audio, block.midi))
    }
}

/// One sine, and where its envelope is.
#[derive(Debug)]
pub struct Tone {
    sample_rate: f32,
    /// What a held note's level gains each frame until it reaches 1.
    attack: f32,
    /// What a released note's level is multiplied by each frame.
    fade: f32,
    velocity: f32,
    /// Its envelope's level, from 0 to 1.
    level: f32,
    released: bool,
    phase: f32,
}

impl Tone {
    fn new(sample_rate: f32) -> Self {
        Tone {
            sample_rate,
            attack: 1.0 / (ATTACK * sample_rate).max(1.0),
            fade: (SILENT.ln() / (RELEASE * sample_rate).max(1.0)).exp(),
            velocity: 0.0,
            level: 0.0,
            released: true,
            phase: 0.0,
        }
    }
}

impl Voice<StereoOut> for Tone {
    fn start(&mut self, note: &NoteOn) {
        (self.velocity, self.level, self.released) = (note.velocity, 0.0, false);
    }

    fn release(&mut self, _: &NoteOff) {
        self.released = true;
    }

    fn render(&mut self, expression: &Expression<'_>, out: &mut VoiceAudio<'_, StereoOut>) {
        let step = expression.pitch.hz() / self.sample_rate;
        let gain = LEVEL * self.velocity * expression.get(VOLUME);
        let octave = 0.5 * expression.pressure;
        for frame in out.frames_mut() {
            if self.released {
                self.level *= self.fade;
                // Silent from the frame it is done, wherever the block ends.
                if self.level < SILENT {
                    self.level = 0.0;
                }
            } else {
                self.level = (self.level + self.attack).min(1.0);
            }
            let angle = self.phase * TAU;
            frame.add((angle.sin() + octave * (2.0 * angle).sin()) * gain * self.level);
            self.phase = (self.phase + step).fract();
        }
    }

    fn is_done(&self) -> bool {
        self.released && self.level == 0.0
    }
}
