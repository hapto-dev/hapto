//! The example synth: sine voices a MIDI keyboard plays.
//!
//! It shows an instrument that plays MIDI through [`Poly`]: `play` cuts the
//! block at each message, in one form whatever the host sent, and gives each
//! note one of the synth's voices. A voice is told when its note starts and
//! when nothing holds it any more, and reads the note's pitch and pressure
//! from its [`Expression`], however the host sent them: a bend, a note's own
//! pitch, MPE or a VST3 host's tuning expression all arrive in the pitch.
//! Each voice renders mono into its [`VoiceAudio`], which places it by the
//! note's pan.
//!
//! Its `cdylib` is a VST3 module through Hapto's exporter (the `vst3`
//! feature, on by default), and the `standalone` feature makes it an app of
//! its own, which plays the computer's MIDI inputs.

#[cfg(feature = "vst3")]
hapto_host::export_vst3!(Synth);

use std::f32::consts::TAU;

use hapto_host::layout::StereoOut;
use hapto_host::midi::{Expression, NoteOff, NoteOn, Poly, Voice, VoiceAudio};
use hapto_host::{Block, Parts, Plugin, PluginInfo, ProcessSetup, Processor, Status};

/// How many notes sound at once.
const VOICES: usize = 16;
/// How long a released note takes to fall 60 dB, in seconds.
const RELEASE: f32 = 0.2;
/// The level at which a released note stops: 60 dB down.
const SILENT: f32 = 1e-3;
/// The level of a note at velocity 1.
const LEVEL: f32 = 0.25;

/// The plugin, on the main thread: it holds nothing.
#[derive(Debug)]
pub struct Synth;

impl Plugin for Synth {
    const INFO: PluginInfo = hapto_host::manifest_info! {
        midi_input: true,
        midi_output: false,
    };

    type Params = ();

    type Processor = SynthProcessor;

    fn new(_: (), _parts: Parts) -> Self {
        Synth
    }

    fn activate(&mut self, setup: &ProcessSetup) -> SynthProcessor {
        let sample_rate = setup.sample_rate as f32;
        SynthProcessor {
            voices: Poly::new(VOICES, || Sine::new(sample_rate)),
        }
    }
}

/// The audio-thread half: the voices, played from the block's MIDI.
#[derive(Debug)]
pub struct SynthProcessor {
    voices: Poly<Sine>,
}

impl Processor for SynthProcessor {
    type Layout = StereoOut;

    fn process(&mut self, block: Block<'_, StereoOut>) -> Status {
        // Sleeps once every voice is silent.
        self.voices.play(block)
    }
}

/// One voice: a sine that fades out once nothing holds its note, with the
/// octave above it as the key is pressed.
#[derive(Debug)]
pub struct Sine {
    sample_rate: f32,
    /// What the level is multiplied by each frame after the release.
    fade: f32,
    phase: f32,
    level: f32,
    released: bool,
}

impl Sine {
    fn new(sample_rate: f32) -> Self {
        Sine {
            sample_rate,
            fade: (SILENT.ln() / (RELEASE * sample_rate)).exp(),
            phase: 0.0,
            level: 0.0,
            released: true,
        }
    }
}

impl Voice for Sine {
    fn start(&mut self, note: &NoteOn) {
        self.level = LEVEL * note.velocity;
        self.released = false;
    }

    fn release(&mut self, _note: &NoteOff) {
        self.released = true;
    }

    fn render(&mut self, expression: &Expression<'_>, out: &mut VoiceAudio<'_>) {
        let step = expression.pitch.hz() / self.sample_rate;
        let octave = 0.5 * expression.pressure;
        for frame in out.frames_mut() {
            let angle = self.phase * TAU;
            // Into both channels, at the note's pan.
            frame.add((angle.sin() + octave * (2.0 * angle).sin()) * self.level);
            self.phase = (self.phase + step).fract();
            if self.released {
                self.level *= self.fade;
                // Silent from the frame it is done, wherever the block ends.
                if self.level < SILENT * LEVEL {
                    self.level = 0.0;
                }
            }
        }
    }

    fn is_done(&self) -> bool {
        self.released && self.level == 0.0
    }
}

#[cfg(test)]
mod tests {
    use hapto_host::AudioBuf;
    use hapto_host::layout::StereoOut;
    use hapto_host::midi::{Centred, Expression, Key, NoteOn, Voice, VoiceAudio};

    use super::Sine;

    #[test]
    fn a_released_sine_fades_until_it_is_done() {
        let mut sine = Sine::new(48_000.0);
        let note = NoteOn::new(Key::A4, 1.0);
        let at_rest = Expression::at(Key::A4);
        let mut buffer = AudioBuf::<StereoOut>::new(4_800);
        let mut render = |sine: &mut Sine| {
            sine.render(
                &at_rest,
                &mut VoiceAudio::new(buffer.audio(), Centred::CENTRE),
            );
        };

        sine.start(&note);
        render(&mut sine);
        assert!(!sine.is_done());

        sine.release(&note.off(0.5));
        render(&mut sine);
        assert!(!sine.is_done(), "0.1 s after the release");
        render(&mut sine);
        render(&mut sine);
        assert!(sine.is_done(), "0.3 s after the release");
    }
}
