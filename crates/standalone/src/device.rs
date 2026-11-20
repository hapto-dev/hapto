//! The audio device: the system's default output, through cpal.

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
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
        let heard = Arc::new(Heard::default());
        let (told, played) = (Arc::clone(&heard), Arc::clone(&heard));
        let stream = self
            .device
            .build_output_stream(
                config,
                move |data: &mut [f32], _| {
                    played.blocks.fetch_add(1, Ordering::Relaxed);
                    player.play(data);
                },
                // cpal reports some errors and plays on, a changed or
                // unplugged default output among them, so this may run
                // beside the blocks: it never waits for the lock, and the
                // error's text is made by whoever polls.
                move |error| {
                    if let Ok(mut last) = told.last.try_lock() {
                        *last = Some(error);
                    }
                    told.errors.fetch_add(1, Ordering::Relaxed);
                },
                None,
            )
            .with_context(|| format!("the audio output device at {rate} Hz"))?;
        stream.play().context("the audio output device")?;
        Ok(Playing {
            _stream: stream,
            heard,
            watch: Watch::default(),
        })
    }
}

/// How long after an error a device is watched before it is said to play on
/// or to have stopped.
const STALLED: Duration = Duration::from_secs(1);

/// What cpal's callbacks tell the thread that polls. The counts are atomics.
/// The error is behind a lock because cpal hands it over by value on its own
/// thread and no atomic holds one; that thread only tries the lock, and the
/// count says an error came even when its text was not kept.
#[derive(Default)]
struct Heard {
    blocks: AtomicU64,
    errors: AtomicU64,
    last: Mutex<Option<cpal::Error>>,
}

/// What a poll of a playing device found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Report {
    /// Nothing to say.
    Quiet,
    /// It reported this error and played on for a second after.
    PlaysOn(String),
    /// It reported this error and played no block for a second after: it
    /// stopped on its own.
    Stopped(String),
}

/// Judges each error a device reports by what follows it. An error alone
/// does not say the device stopped, since cpal reports a changed default
/// output and plays on: the blocks of the next second say which it was.
#[derive(Default)]
struct Watch {
    /// How many errors were reported at the last look.
    errors: u64,
    /// The error being judged: its text, the blocks played when it was
    /// seen, and when that was.
    judging: Option<(String, u64, Instant)>,
}

impl Watch {
    /// Looks at the counts at `now`. A new error starts a second's watch,
    /// with the text `why` makes; a watch that has run its second ends in
    /// [`Report::PlaysOn`] if a block was played in it, and in
    /// [`Report::Stopped`] if none was. Blocks that stop with no error
    /// reported say nothing.
    fn look(
        &mut self,
        now: Instant,
        errors: u64,
        blocks: u64,
        why: impl FnOnce() -> String,
    ) -> Report {
        if errors != self.errors {
            self.errors = errors;
            self.judging = Some((why(), blocks, now));
        }
        let Some((why, seen, since)) = self.judging.take() else {
            return Report::Quiet;
        };
        if now.duration_since(since) < STALLED {
            self.judging = Some((why, seen, since));
            Report::Quiet
        } else if blocks == seen {
            Report::Stopped(why)
        } else {
            Report::PlaysOn(why)
        }
    }
}

/// The device playing a player's blocks, until it drops.
pub(crate) struct Playing {
    _stream: cpal::Stream,
    heard: Arc<Heard>,
    watch: Watch,
}

impl Playing {
    /// Polls the device: whether an error it reported was followed by a
    /// second of blocks or by none.
    pub(crate) fn poll(&mut self) -> Report {
        let heard = &self.heard;
        self.watch.look(
            Instant::now(),
            heard.errors.load(Ordering::Relaxed),
            heard.blocks.load(Ordering::Relaxed),
            || {
                let last = heard.last.lock().ok().and_then(|mut last| last.take());
                last.map_or_else(
                    || "an error whose text was not kept".to_owned(),
                    |error| error.to_string(),
                )
            },
        )
    }
}
