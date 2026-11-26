//! The test signal the app plays into a plugin's inputs.

#[cfg(test)]
mod tests;

/// The test signals' level: -24 dBFS, far enough below full scale that a
/// gain of 24 dB does not clip.
pub(crate) const LEVEL: f32 = 0.063_095_73;

/// The sine's frequency, in hertz.
const SINE_HZ: f64 = 440.0;

/// A test signal: silence unless asked for, so nothing sounds until the
/// plugin's author wants it to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    /// A 440 Hz sine at -24 dBFS, `LEVEL`.
    Sine,
    /// White noise at -24 dBFS, `LEVEL`, peak.
    Noise,
    /// Nothing.
    Silence,
}

impl Signal {
    /// Every signal, in the order the settings window lists them.
    #[cfg(any(feature = "dev-host", test))]
    pub(crate) const ALL: [Signal; 3] = [Signal::Silence, Signal::Sine, Signal::Noise];

    /// Returns the signal `name` names on the command line or in a message.
    pub(crate) fn named(name: &str) -> Option<Signal> {
        match name {
            "sine" => Some(Signal::Sine),
            "noise" => Some(Signal::Noise),
            "silence" => Some(Signal::Silence),
            _ => None,
        }
    }

    /// Returns the name the command line and the settings window's messages
    /// use.
    #[cfg(any(feature = "dev-host", test))]
    pub(crate) fn name(self) -> &'static str {
        match self {
            Signal::Sine => "sine",
            Signal::Noise => "noise",
            Signal::Silence => "silence",
        }
    }

    /// Returns the name a person reads.
    #[cfg(all(hapto_app, feature = "dev-host"))]
    pub(crate) fn label(self) -> &'static str {
        match self {
            Signal::Sine => "Sine, 440 Hz",
            Signal::Noise => "Noise",
            Signal::Silence => "Silence",
        }
    }

    /// Returns its number, for an atomic the audio thread reads.
    pub(crate) fn index(self) -> u8 {
        match self {
            Signal::Silence => 0,
            Signal::Sine => 1,
            Signal::Noise => 2,
        }
    }

    /// Returns the signal whose number is `index`; silence for any other.
    pub(crate) fn from_index(index: u8) -> Signal {
        match index {
            1 => Signal::Sine,
            2 => Signal::Noise,
            _ => Signal::Silence,
        }
    }
}

/// Writes a signal, block after block, with no gap between blocks.
#[derive(Clone, Debug)]
pub(crate) struct Generator {
    signal: Signal,
    /// The sine's phase, in turns, from 0 to 1.
    phase: f64,
    /// How far the phase moves each frame.
    step: f64,
    /// The noise's state: a 32-bit xorshift, never 0.
    noise: u32,
}

impl Generator {
    /// Writes `signal` at `sample_rate`.
    pub(crate) fn new(signal: Signal, sample_rate: f64) -> Self {
        Generator {
            signal,
            phase: 0.0,
            step: SINE_HZ / sample_rate,
            noise: 0x9e37_79b9,
        }
    }

    /// Writes `signal` from the next frame on, the sine's phase carrying on.
    pub(crate) fn set(&mut self, signal: Signal) {
        self.signal = signal;
    }

    /// Writes the signal's next frames into `out`.
    pub(crate) fn fill(&mut self, out: &mut [f32]) {
        match self.signal {
            Signal::Silence => out.fill(0.0),
            Signal::Sine => {
                for sample in out {
                    *sample = LEVEL * (std::f64::consts::TAU * self.phase).sin() as f32;
                    self.phase = (self.phase + self.step).fract();
                }
            }
            Signal::Noise => {
                for sample in out {
                    let mut x = self.noise;
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    self.noise = x;
                    // The top 24 bits, as a float from -1 to 1.
                    let unit = (x >> 8) as f32 / (1 << 23) as f32 - 1.0;
                    *sample = LEVEL * unit;
                }
            }
        }
    }
}
