//! The audio device: the system's default output, through cpal.

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use cpal::ErrorKind;
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
        let told = Arc::clone(&heard);
        let stream = self
            .device
            .build_output_stream(
                config,
                move |data: &mut [f32], _| player.play(data),
                // On cpal's own thread, which may be the one that plays the
                // blocks: it never waits for the lock, and the error's text
                // is made by whoever reads it.
                move |error| {
                    let kind = kind_index(error.kind());
                    if let Ok(mut last) = told.last.try_lock() {
                        *last = Some(error);
                    }
                    told.kind.store(kind, Ordering::Relaxed);
                    told.errors.fetch_add(1, Ordering::Release);
                },
                None,
            )
            .with_context(|| format!("the audio output device at {rate} Hz"))?;
        stream.play().context("the audio output device")?;
        Ok(Playing {
            _stream: stream,
            heard,
            read: 0,
        })
    }
}

/// What a device's error asks of the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    /// The device plays on: a glitch, a refused rise in the thread's
    /// priority, or a change of route it followed itself.
    PlaysOn,
    /// The stream has ended, or plays to a device that is no longer the
    /// default output: the default is opened again.
    Reopen,
}

/// Returns what an error of `kind` asks of the app. On Windows every other
/// error either ends cpal's thread or says the stream must be built again,
/// since cpal never moves a stream to a new default output: it reports the
/// change and goes on playing to the old one.
pub(crate) fn answer(kind: ErrorKind) -> Answer {
    match kind {
        ErrorKind::Xrun | ErrorKind::RealtimeDenied | ErrorKind::DeviceChanged => Answer::PlaysOn,
        _ => Answer::Reopen,
    }
}

/// How long after the default output is opened again a second error that
/// asks for it stops the audio instead, so a device that fails at once is
/// not opened again at every tick.
pub(crate) const SETTLE: Duration = Duration::from_secs(1);

/// When the default output was last opened again after an error.
#[derive(Debug, Default)]
pub(crate) struct Reopens {
    last: Option<Instant>,
}

impl Reopens {
    /// Whether an error at `now` that asks for the default output may open
    /// it again: not within [`SETTLE`] of the last time. One that may is
    /// counted as made.
    pub(crate) fn may(&mut self, now: Instant) -> bool {
        if self
            .last
            .is_some_and(|last| now.duration_since(last) < SETTLE)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// An error a device reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceError {
    /// What kind it is, which says what it asks of the app.
    pub(crate) kind: ErrorKind,
    /// Its text: cpal's own, or its kind's when cpal's was not kept.
    pub(crate) text: String,
}

/// Every kind of error cpal names, so the error callback keeps one as its
/// place here, in an atomic. A kind a later cpal adds reads as `Other`.
const KINDS: [ErrorKind; 14] = [
    ErrorKind::DeviceBusy,
    ErrorKind::DeviceChanged,
    ErrorKind::DeviceNotAvailable,
    ErrorKind::HostUnavailable,
    ErrorKind::InvalidInput,
    ErrorKind::PermissionDenied,
    ErrorKind::RealtimeDenied,
    ErrorKind::ResourceExhausted,
    ErrorKind::StreamInvalidated,
    ErrorKind::UnsupportedConfig,
    ErrorKind::UnsupportedOperation,
    ErrorKind::Xrun,
    ErrorKind::BackendError,
    ErrorKind::Other,
];

/// Returns `kind`'s place in [`KINDS`], or past its end for a kind it does
/// not hold.
fn kind_index(kind: ErrorKind) -> u8 {
    KINDS
        .iter()
        .position(|known| *known == kind)
        .and_then(|place| u8::try_from(place).ok())
        .unwrap_or(u8::MAX)
}

/// Returns the kind at `index` in [`KINDS`], `Other` past its end.
fn kind_at(index: u8) -> ErrorKind {
    KINDS
        .get(usize::from(index))
        .copied()
        .unwrap_or(ErrorKind::Other)
}

/// What cpal's error callback tells the thread that reads it: how many
/// errors came, and the newest one's kind, in atomics, so neither is ever
/// lost. The error is behind a lock because cpal hands it over by value on
/// its own thread and no atomic holds one; that thread only tries the lock,
/// so its text may be lost, never its kind.
#[derive(Default)]
struct Heard {
    errors: AtomicU64,
    kind: AtomicU8,
    last: Mutex<Option<cpal::Error>>,
}

/// The device playing a player's blocks, until it drops.
pub(crate) struct Playing {
    _stream: cpal::Stream,
    heard: Arc<Heard>,
    /// How many errors had come at the last read.
    read: u64,
}

impl Playing {
    /// Returns the newest error the device reported since the last call,
    /// and clears it.
    pub(crate) fn error(&mut self) -> Option<DeviceError> {
        let heard = &self.heard;
        // Pairs with the error callback's count, so the kind is at least the
        // counted error's.
        let errors = heard.errors.load(Ordering::Acquire);
        if errors == self.read {
            return None;
        }
        self.read = errors;
        let kind = kind_at(heard.kind.load(Ordering::Relaxed));
        let last = heard.last.lock().ok().and_then(|mut last| last.take());
        // A text kept from an older error than the newest is not its text.
        let text = last
            .filter(|error| error.kind() == kind)
            .map_or_else(|| kind.to_string(), |error| error.to_string());
        Some(DeviceError { kind, text })
    }
}
