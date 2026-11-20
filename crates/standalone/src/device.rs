//! The audio device: the system's default output, through cpal.

#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

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
        let reported = Arc::new(OnceLock::new());
        let blocks = Arc::new(AtomicU64::new(0));
        let (told, played) = (Arc::clone(&reported), Arc::clone(&blocks));
        let stream = self
            .device
            .build_output_stream(
                config,
                move |data: &mut [f32], _| {
                    played.fetch_add(1, Ordering::Relaxed);
                    player.play(data);
                },
                // cpal reports some errors and plays on, a change of the
                // system's default output among them, so this may run beside
                // the blocks: the error is kept as it is, and its text is
                // made by whoever asks.
                move |error| {
                    let _first = told.set(error);
                },
                None,
            )
            .with_context(|| format!("the audio output device at {rate} Hz"))?;
        stream.play().context("the audio output device")?;
        Ok(Playing {
            _stream: stream,
            reported,
            blocks,
            last: Cell::new((0, Instant::now())),
            said: Cell::new(false),
        })
    }
}

/// How long a device that reported an error may play no block before it is
/// taken to have stopped.
const STALLED: Duration = Duration::from_secs(1);

/// Whether a device has stopped playing on its own: it reported an error,
/// and has played no block for `quiet`. An error alone does not say so,
/// since cpal reports a changed default output and plays on.
fn stopped(reported: bool, quiet: Duration) -> bool {
    reported && quiet >= STALLED
}

/// The device playing a player's blocks, until it drops.
pub(crate) struct Playing {
    _stream: cpal::Stream,
    /// The first error the device reported.
    reported: Arc<OnceLock<cpal::Error>>,
    /// How many blocks it has played.
    blocks: Arc<AtomicU64>,
    /// The count [`ended`](Self::ended) last saw move, and when.
    last: Cell<(u64, Instant)>,
    /// Whether the error was said in a line.
    said: Cell<bool>,
}

impl Playing {
    /// Returns why the device stopped playing on its own, once it has: it
    /// reported an error and then played no block for a second. An error it
    /// plays on after is said once in a line, and the app goes on.
    pub(crate) fn ended(&self) -> Option<String> {
        let now = Instant::now();
        let blocks = self.blocks.load(Ordering::Relaxed);
        let (seen, since) = self.last.get();
        if blocks != seen {
            self.last.set((blocks, now));
        }
        let error = self.reported.get()?;
        if !self.said.replace(true) {
            eprintln!("the audio output reported an error and plays on: {error}");
        }
        let quiet = if blocks == seen {
            now.duration_since(since)
        } else {
            Duration::ZERO
        };
        stopped(true, quiet).then(|| error.to_string())
    }
}
