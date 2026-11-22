//! The example EQ: six biquad bands (a high-pass, a low shelf, three peaks
//! and a high shelf) and an output gain, on Hapto.
//!
//! It has parameters and saved state only, with no editor, undo or MIDI
//! learn, so it can be measured against the same EQ written straight in
//! JUCE. Its `cdylib` is a VST3 module through Hapto's exporter (the `vst3`
//! feature, on by default); with only `cabi` on, it exports the C interface
//! instead, for the JUCE adapter.
//!
//! A band retunes at the start of the first block after one of its
//! parameters moves. A band that is off keeps running and passes its input
//! through, as a bypassed filter in a JUCE `ProcessorChain` does, so it comes
//! back on warm.

pub mod dsp;
pub mod params;

#[cfg(feature = "vst3")]
hapto_host::export_vst3!(ExampleEq);
#[cfg(feature = "cabi")]
hapto_host::export_cabi!(ExampleEq);

use hapto_host::layout::{AnyLayout, Width};
use hapto_host::{Block, Parts, Plugin, PluginInfo, ProcessSetup, Processor, Status};
use hapto_params::{Bool, Changes, Float};

use crate::dsp::{Biquad, coefficients, decibels_to_gain};
use crate::params::{BandParams, EqParams};

/// The plugin: its parameters, on the main thread. It drops the rest of its
/// parts, so it stays `Send`.
#[derive(Debug)]
pub struct ExampleEq {
    params: EqParams,
}

// The C interface binds a plugin that is `Send` to no thread. One that
// stopped being `Send` would be bound to its creating thread instead, with no
// error, so this fails the build first.
const _: () = {
    const fn send<T: Send>() {}
    send::<ExampleEq>();
};

impl Plugin for ExampleEq {
    const INFO: PluginInfo = hapto_host::manifest_info! {
        // Its processor takes `AnyLayout`, so these are the widths it runs at.
        layouts: &[Width::STEREO, Width::MONO],
        midi_input: false,
        midi_output: false,
    };

    type Params = EqParams;

    type Processor = EqProcessor;

    fn new(params: EqParams, _parts: Parts) -> Self {
        ExampleEq { params }
    }

    fn activate(&mut self, setup: &ProcessSetup) -> EqProcessor {
        EqProcessor::new(self.params.clone(), setup.sample_rate)
    }
}

/// The audio-thread half: six filters and a gain over the first two
/// channels; any further channels pass through.
#[derive(Debug)]
pub struct EqProcessor {
    sample_rate: f64,
    bands: [BandState; 6],
    output: OutputState,
}

#[derive(Debug)]
struct BandState {
    params: BandParams,
    /// What the filter's coefficients were last made from.
    changes: Changes,
    bypassed: bool,
    filter: Biquad,
}

#[derive(Debug)]
struct OutputState {
    on: Bool,
    gain_db: Float,
    changes: Changes,
    /// `None` while the output gain is off.
    gain: Option<f32>,
}

impl EqProcessor {
    fn new(params: EqParams, sample_rate: f64) -> Self {
        let bands = params.bands.map(|band| BandState {
            params: band,
            changes: Changes::new(),
            bypassed: false,
            filter: Biquad::default(),
        });
        let mut processor = EqProcessor {
            sample_rate,
            bands,
            output: OutputState {
                on: params.output_on,
                gain_db: params.output_gain,
                changes: Changes::new(),
                gain: None,
            },
        };
        processor.retune();
        processor
    }

    /// Retunes what moved since it last tuned. Reads atomics only.
    #[cfg_attr(
        not(test),
        deny(
            clippy::indexing_slicing,
            clippy::unwrap_used,
            clippy::expect_used,
            clippy::panic,
            clippy::arithmetic_side_effects
        )
    )]
    fn retune(&mut self) {
        for band in &mut self.bands {
            let params = &band.params;
            let (on, frequency, q) = (&params.on, &params.frequency, &params.q);
            let moved = match &params.gain {
                Some(gain) => band.changes.moved([on, frequency, q, gain]),
                None => band.changes.moved([on, frequency, q]),
            };
            if !moved {
                continue;
            }
            band.bypassed = !on.get();
            let gain_db = params.gain.as_ref().map_or(0.0, Float::get);
            band.filter.coefficients = coefficients(
                params.shape,
                gain_db,
                frequency.get(),
                q.get(),
                self.sample_rate,
            );
        }
        let output = &mut self.output;
        if output.changes.moved([&output.on, &output.gain_db]) {
            output.gain = output
                .on
                .get()
                .then(|| decibels_to_gain(output.gain_db.get()));
        }
    }
}

#[cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects
    )
)]
impl Processor for EqProcessor {
    type Layout = AnyLayout;

    fn process(&mut self, mut block: Block<'_, AnyLayout>) -> Status {
        self.retune();
        for band in &mut self.bands {
            band.filter.process(&mut block.audio, band.bypassed);
        }
        if let Some(gain) = self.output.gain {
            for channel in block.audio.channels_mut().take(2) {
                for sample in channel {
                    *sample *= gain;
                }
            }
        }
        Status::Continue
    }

    fn reset(&mut self) {
        for band in &mut self.bands {
            band.filter.reset();
        }
    }
}
