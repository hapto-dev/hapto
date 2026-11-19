//! The audio device: the system's default output, through cpal.

use std::sync::{Arc, OnceLock};

use anyhow::{Context as _, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::block::{Player, Render};

/// The system's default output device and the stream configuration it
/// prefers.
pub(crate) struct Output {
    device: cpal::Device,
    config: cpal::StreamConfig,
}

impl Output {
    /// Opens the system's default output device in its own configuration.
    ///
    /// # Errors
    ///
    /// Returns why there is none to play to: no device, or one that does not
    /// take 32-bit floats, which shared-mode devices on Windows do.
    pub(crate) fn system_default() -> Result<Output> {
        let device = cpal::default_host()
            .default_output_device()
            .context("there is no audio output device")?;
        let supported = device
            .default_output_config()
            .context("the audio output device")?;
        if supported.sample_format() != cpal::SampleFormat::F32 {
            bail!(
                "the audio output device takes {:?} samples, and the app plays 32-bit floats",
                supported.sample_format()
            );
        }
        Ok(Output {
            device,
            config: supported.config(),
        })
    }

    /// Returns the device's own sample rate.
    pub(crate) fn sample_rate(&self) -> u32 {
        self.config.sample_rate
    }

    /// Returns how many channels the device plays.
    pub(crate) fn channels(&self) -> usize {
        usize::from(self.config.channels)
    }

    /// Returns the device's name, as the system shows it.
    #[cfg(feature = "dev-host")]
    pub(crate) fn name(&self) -> String {
        self.device
            .description()
            .map(|d| d.name().to_owned())
            .unwrap_or_else(|_| "the default output".to_owned())
    }

    /// Returns the standard sample rates the device takes, at its own
    /// channel count and in 32-bit floats, and its own rate. On Windows the
    /// system converts a stream at any of them to the device's own rate.
    #[cfg(feature = "dev-host")]
    pub(crate) fn rates(&self) -> Vec<u32> {
        let ranges = self
            .device
            .supported_output_configs()
            .into_iter()
            .flatten()
            .filter(|range| {
                range.sample_format() == cpal::SampleFormat::F32
                    && range.channels() == self.config.channels
            })
            .map(|range| (range.min_sample_rate(), range.max_sample_rate()));
        crate::settings::offered(ranges, self.sample_rate())
    }

    /// Starts the device at `rate`, playing `player`'s blocks, until the
    /// stream it returns drops. Once it has dropped, no more of the
    /// player's blocks run.
    ///
    /// # Errors
    ///
    /// Returns why the device did not start.
    pub(crate) fn start<R: Render>(&self, rate: u32, mut player: Player<R>) -> Result<Playing> {
        let config = cpal::StreamConfig {
            sample_rate: rate,
            ..self.config
        };
        let ended = Arc::new(OnceLock::new());
        let told = Arc::clone(&ended);
        let stream = self
            .device
            .build_output_stream(
                config,
                move |data: &mut [f32], _| player.play(data),
                // On Windows cpal's stream ends once it reports an error, so
                // its blocks are over by then, and the text may be made here.
                move |error| {
                    let _first = told.set(error.to_string());
                },
                None,
            )
            .with_context(|| format!("the audio output device at {rate} Hz"))?;
        stream.play().context("the audio output device")?;
        Ok(Playing {
            _stream: stream,
            ended,
        })
    }
}

/// The device playing a player's blocks, until it drops.
pub(crate) struct Playing {
    _stream: cpal::Stream,
    /// Why the device stopped playing on its own, set once as it stops.
    ended: Arc<OnceLock<String>>,
}

impl Playing {
    /// Returns why the device stopped playing on its own, once it has: the
    /// device unplugged, say.
    pub(crate) fn ended(&self) -> Option<&str> {
        self.ended.get().map(String::as_str)
    }
}
