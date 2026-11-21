//! The audio device: the system's default output, through cpal, and the rule
//! that follows it from device to device.

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{DeviceId, ErrorKind};

use crate::block::{Player, Render};

/// The device that was the system's default output when it was opened, and
/// the stream configuration it prefers.
pub(crate) struct Output {
    device: cpal::Device,
    config: cpal::StreamConfig,
    id: DeviceId,
}

impl Output {
    /// Opens the device that is the system's default output now, taken from
    /// the host by its id, in its own configuration. Its stream stays on that
    /// device and tells of no change of the default, which
    /// [`default_output_id`] says.
    ///
    /// cpal's own default device is not used: on Windows it cannot be opened
    /// again once any audio device has left during the run.
    ///
    /// # Errors
    ///
    /// Returns why there is none to play to: no device, a default that is
    /// not found again by its id, or one that does not take 32-bit floats,
    /// which shared-mode devices on Windows do.
    pub(crate) fn system_default() -> Result<Output> {
        let id = default_output_id().context("there is no audio output device")?;
        let device = cpal::default_host()
            .device_by_id(&id)
            .context("there is no audio output device: none has the default's id")?;
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
            id,
        })
    }

    /// Returns the id it was opened by.
    pub(crate) fn id(&self) -> &DeviceId {
        &self.id
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

/// Returns the id of the device that is the system's default output now,
/// `None` when there is none. It opens nothing and builds no stream.
pub(crate) fn default_output_id() -> Option<DeviceId> {
    cpal::default_host().default_output_device()?.id().ok()
}

/// How often the app and the dev host look at which device is the system's
/// default output.
pub(crate) const LOOK: Duration = Duration::from_secs(1);

/// What a look at the system's default output asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Look {
    /// Play on: the device that plays is the default.
    Keep,
    /// Stop the device that plays, then open the default: another device is
    /// the default now.
    Move,
    /// Stop: there is no default output.
    Stop,
    /// Open the default: nothing plays, and there is one.
    Open,
    /// Wait: nothing plays, and there is no default to open.
    Wait,
}

/// Returns what a look asks of audio that plays on the device `playing`, or
/// on none, when the system's default output is `default`, or none.
///
/// Audio an error stopped plays on none, so the next look opens the default
/// again: no device is opened more than once a look, and no device error
/// stops the audio for the run.
pub(crate) fn look<Id: PartialEq>(playing: Option<&Id>, default: Option<&Id>) -> Look {
    match (playing, default) {
        (Some(playing), Some(default)) if playing == default => Look::Keep,
        (Some(_), Some(_)) => Look::Move,
        (Some(_), None) => Look::Stop,
        (None, Some(_)) => Look::Open,
        (None, None) => Look::Wait,
    }
}

/// Returns `reason` unless `said` holds it already, and holds it there: a
/// failure that repeats at every look is said once, and again only when its
/// reason changes.
pub(crate) fn say_once(said: &mut Option<String>, reason: String) -> Option<String> {
    if said.as_ref() == Some(&reason) {
        return None;
    }
    *said = Some(reason.clone());
    Some(reason)
}

/// What a device's error asks of the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    /// The device plays on: a glitch, a refused rise in the thread's
    /// priority, or a change of route it followed itself.
    PlaysOn,
    /// The stream has ended, or must be built again: it is dropped at once,
    /// and the next look opens the default output again.
    Reopen,
}

/// Returns what an error of `kind` asks of the app. On Windows every other
/// error either ends cpal's thread or says the stream must be built again.
pub(crate) fn answer(kind: ErrorKind) -> Answer {
    match kind {
        ErrorKind::Xrun | ErrorKind::RealtimeDenied | ErrorKind::DeviceChanged => Answer::PlaysOn,
        _ => Answer::Reopen,
    }
}

/// An error a device reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceError {
    /// What kind it is, which says what it asks of the app.
    pub(crate) kind: ErrorKind,
    /// Its text: plain words for a device that went away, else cpal's own,
    /// or its kind's when cpal's was not kept.
    pub(crate) text: String,
}

/// Returns the text of an error of `kind`, given the error the callback
/// `kept`. A device that went away is said in plain words, since cpal's
/// text names a call of the system's and a number; every other kind keeps
/// cpal's text.
fn text(kind: ErrorKind, kept: Option<cpal::Error>) -> String {
    if kind == ErrorKind::DeviceNotAvailable {
        return "the audio device went away".to_owned();
    }
    // A text kept from an older error than the newest is not its text.
    kept.filter(|error| error.kind() == kind)
        .map_or_else(|| kind.to_string(), |error| error.to_string())
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
        let kept = heard.last.lock().ok().and_then(|mut last| last.take());
        Some(DeviceError {
            kind,
            text: text(kind, kept),
        })
    }
}
