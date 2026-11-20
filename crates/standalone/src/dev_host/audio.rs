//! The dev host's audio: the device, the settings the plugin runs with, and
//! a change of them as a host makes one.
//!
//! A host changes the sample rate or the most frames a block holds with no
//! block running: it stops processing, deactivates the plugin, sets it up
//! again and activates it. Here the device's stream is dropped first, which
//! ends its blocks, and started again at the new rate once the plugin is
//! active. A change of how blocks are cut, or of the test signal, reaches
//! the audio thread at once, with no reactivation, as a host's own cut
//! varies from block to block. Stopped, the plugin is deactivated and no
//! block runs; a change of settings then waits for the start.
//!
//! An error from the device that it does not play on after, a new default
//! output among them, opens the system's default output again, as a host
//! follows a change of device: the plugin is stopped, then started on the
//! new device.

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use hapto_host::Layout;

use super::hosted::Hosted;
use super::message::AudioShown;
use crate::block::{Plan, Player, Render};
use crate::device::{Answer, DeviceError, Output, Playing, Reopens, answer};
use crate::settings::{MAX_BLOCKS, Settings};
use crate::signal::Signal;

/// An audio output the dev host plays the plugin to: the system's default
/// output, or a test's.
pub(crate) trait Device {
    /// What plays the blocks, until it drops.
    type Stream;
    /// Returns its name, as the system shows it.
    fn name(&self) -> String;
    /// Returns its own sample rate.
    fn sample_rate(&self) -> u32;
    /// Returns how many channels it plays.
    fn channels(&self) -> usize;
    /// Returns the sample rates it takes, which alone are offered.
    fn rates(&self) -> Vec<u32>;
    /// Starts it at `rate`, playing `player`'s blocks until the stream it
    /// returns drops.
    ///
    /// # Errors
    ///
    /// Returns why it did not start.
    fn start(&self, rate: u32, player: Player<Box<dyn Render>>) -> Result<Self::Stream>;
    /// Returns the newest error `stream` reported since the last call.
    fn error(stream: &mut Self::Stream) -> Option<DeviceError>;
    /// Opens the system's default output again, which may be another
    /// device now.
    ///
    /// # Errors
    ///
    /// Returns why there is none to play to.
    fn reopen(&self) -> Result<Self>
    where
        Self: Sized;
}

impl Device for Output {
    type Stream = Playing;

    fn name(&self) -> String {
        Output::name(self)
    }

    fn sample_rate(&self) -> u32 {
        Output::sample_rate(self)
    }

    fn channels(&self) -> usize {
        Output::channels(self)
    }

    fn rates(&self) -> Vec<u32> {
        Output::rates(self)
    }

    fn start(&self, rate: u32, player: Player<Box<dyn Render>>) -> Result<Playing> {
        Output::start(self, rate, player)
    }

    fn error(stream: &mut Playing) -> Option<DeviceError> {
        stream.error()
    }

    fn reopen(&self) -> Result<Output> {
        Output::system_default()
    }
}

/// The device, the settings the plugin runs with, and the stream playing it.
pub(crate) struct Audio<D: Device = Output> {
    output: D,
    device: String,
    /// The sample rates the device takes, which alone are offered.
    rates: Vec<u32>,
    layout: Layout,
    plan: Arc<Plan>,
    settings: Settings,
    /// The settings the plugin last started with, which a start it refuses
    /// brings back.
    ran: Option<Settings>,
    /// The stream playing the plugin's blocks, while it processes.
    stream: Option<D::Stream>,
    /// Why the last change did not take.
    note: Option<String>,
    /// What changed since the last look, for the Activity log.
    events: Vec<String>,
    reopens: Reopens,
}

impl Audio {
    /// Opens the system's default output for a plugin laid out as `layout`,
    /// with `signal` on its inputs and `kept`, the settings a session kept,
    /// where the device still takes them, and else its own rate.
    ///
    /// # Errors
    ///
    /// Returns why there is no device to play to.
    pub(crate) fn open(layout: Layout, signal: Signal, kept: Option<&str>) -> Result<Audio> {
        Ok(Audio::with(Output::system_default()?, layout, signal, kept))
    }
}

impl<D: Device> Audio<D> {
    /// Plays to `output`, as [`open`](Audio::open) does to the system's.
    pub(crate) fn with(output: D, layout: Layout, signal: Signal, kept: Option<&str>) -> Self {
        let rates = output.rates();
        let settings = kept
            .and_then(|text| Settings::read(text, &rates))
            .unwrap_or_else(|| Settings::starting(output.sample_rate()));
        Audio {
            device: output.name(),
            plan: Plan::new(settings.blocks, signal),
            output,
            rates,
            layout,
            settings,
            ran: None,
            stream: None,
            note: None,
            events: Vec::new(),
            reopens: Reopens::default(),
        }
    }

    /// Returns the settings the plugin runs with.
    pub(crate) fn settings(&self) -> Settings {
        self.settings
    }

    /// Whether the plugin processes: not after [`stop`](Self::stop), nor
    /// after a start it refused.
    pub(crate) fn is_processing(&self) -> bool {
        self.stream.is_some()
    }

    /// Returns the settings the plugin processes with, the ones a session
    /// keeps; `None` while it does not process.
    pub(crate) fn running(&self) -> Option<Settings> {
        self.is_processing().then_some(self.settings)
    }

    /// Starts the plugin for the first time in the run. Settings a session
    /// kept that the plugin refuses give way to the device's own. A start
    /// that fails even so leaves processing stopped, with a note that says
    /// why, so no refusal keeps the dev host from starting and showing it.
    pub(crate) fn start_first(&mut self, hosted: &dyn Hosted) {
        let Err(error) = self.start(hosted) else {
            return;
        };
        let own = Settings::starting(self.output.sample_rate());
        let note = if self.settings == own {
            format!("{error:#}: processing is stopped")
        } else {
            self.settings = own;
            self.plan.set(own.blocks);
            match self.start(hosted) {
                Ok(()) => format!("{error:#}: the device's own settings are used instead"),
                Err(again) => format!(
                    "{error:#}; the device's own settings did not start either: {again:#}: \
                     processing is stopped"
                ),
            }
        };
        self.events.push(format!("refused: {note}"));
        self.note = Some(note);
    }

    /// Activates the plugin for the settings and starts the device playing
    /// it. A start that fails part of the way, the plugin active and the
    /// device not playing it, is undone: the plugin is stopped again.
    ///
    /// # Errors
    ///
    /// Returns the call the plugin refused, or why the device did not start.
    fn start(&mut self, hosted: &dyn Hosted) -> Result<()> {
        let started = self.play(hosted);
        if started.is_err() {
            hosted.stop();
        }
        started
    }

    /// Activates the plugin and starts the device playing it, as
    /// [`start`](Self::start) does, but for the undoing.
    fn play(&mut self, hosted: &dyn Hosted) -> Result<()> {
        let Settings { rate, max, .. } = self.settings;
        let render = hosted.start(f64::from(rate), max, self.layout)?;
        let channels = self.output.channels();
        let plan = Arc::clone(&self.plan);
        let player = Player::new(render, self.layout, channels, f64::from(rate), max, plan);
        self.stream = Some(self.output.start(rate, player)?);
        self.ran = Some(self.settings);
        Ok(())
    }

    /// Changes the settings to `wanted`, as a host does: a new rate or
    /// maximum reactivates the plugin, a new cut does not. Settings the
    /// device or the dev host does not offer are passed over.
    pub(crate) fn change(&mut self, hosted: &dyn Hosted, wanted: Settings) {
        if !self.rates.contains(&wanted.rate) || !MAX_BLOCKS.contains(&wanted.max) {
            return;
        }
        let before = self.settings;
        if wanted == before {
            return;
        }
        self.settings = wanted;
        self.plan.set(wanted.blocks);
        if wanted.blocks != before.blocks {
            self.events
                .push(format!("blocks: {}", wanted.blocks.label()));
        }
        if (wanted.rate, wanted.max) != (before.rate, before.max) && self.is_processing() {
            self.restart(hosted, Some(before));
        }
    }

    /// Reactivates the plugin with the settings it has, as a host does when
    /// the user asks it to, or reloads the plugin's state. Stopped, it
    /// starts; settings changed meanwhile that it refuses give way to the
    /// ones that last ran.
    pub(crate) fn reactivate(&mut self, hosted: &dyn Hosted) {
        let before = self.ran.filter(|ran| *ran != self.settings);
        self.restart(hosted, before);
    }

    /// Stops processing and deactivates the plugin, with no block running,
    /// until [`resume`](Self::resume).
    pub(crate) fn stop(&mut self, hosted: &dyn Hosted) {
        if !self.is_processing() {
            return;
        }
        self.silence();
        hosted.stop();
        self.note = None;
        self.events.push("processing stopped".to_owned());
    }

    /// Activates the plugin again after [`stop`](Self::stop), with the
    /// settings it has now, as [`reactivate`](Self::reactivate) does.
    pub(crate) fn resume(&mut self, hosted: &dyn Hosted) {
        if !self.is_processing() {
            self.reactivate(hosted);
        }
    }

    /// Plays `signal` into the plugin's inputs from the next block.
    pub(crate) fn set_signal(&mut self, signal: Signal) {
        if self.plan.signal() != signal {
            self.plan.set_signal(signal);
            self.events.push(format!("input: {}", signal.label()));
        }
    }

    /// Stops the plugin with no block running, and starts it again with the
    /// settings now. When it does not start, the settings `before`, where
    /// given, come back, and the note says why.
    fn restart(&mut self, hosted: &dyn Hosted, before: Option<Settings>) {
        self.silence();
        hosted.stop();
        self.note = None;
        let Err(error) = self.start(hosted) else {
            self.events.push(format!("activated at {}", self.shape()));
            return;
        };
        let Some(before) = before else {
            let note = format!("{error:#}");
            self.events.push(format!("refused: {note}"));
            self.note = Some(note);
            return;
        };
        self.settings = before;
        self.plan.set(before.blocks);
        let note = match self.start(hosted) {
            Ok(()) => format!("{error:#}: the settings before are back"),
            Err(again) => format!("{error:#}; the settings before did not start either: {again:#}"),
        };
        self.events.push(format!("refused: {note}"));
        self.note = Some(note);
    }

    /// Reads the device's newest error at `now`. One it plays on after goes
    /// in the log; any other [reopens](Self::reopen) the system's default
    /// output.
    pub(crate) fn check(&mut self, hosted: &dyn Hosted, now: Instant) {
        let Some(error) = self.stream.as_mut().and_then(D::error) else {
            return;
        };
        match answer(error.kind) {
            Answer::PlaysOn => self.events.push(format!(
                "the audio device reported an error and plays on: {}",
                error.text
            )),
            Answer::Reopen => self.reopen(hosted, &error.text, now),
        }
    }

    /// Stops processing, as a host stops for a change of device, and starts
    /// it again on the system's default output, with the settings it has
    /// where the device offers their rate, and else at the device's own.
    /// A device that does not open, or a second error within
    /// [`SETTLE`](crate::device::SETTLE) of the last reopen, leaves
    /// processing stopped, and the note and the log say why.
    fn reopen(&mut self, hosted: &dyn Hosted, why: &str, now: Instant) {
        self.silence();
        hosted.stop();
        if !self.reopens.may(now) {
            self.stopped(format!(
                "processing is stopped: the audio device failed again within a second of \
                 opening it, after an error: {why}"
            ));
            return;
        }
        let output = match self.output.reopen() {
            Ok(output) => output,
            Err(error) => {
                self.stopped(format!(
                    "processing is stopped: the default output did not open again ({error:#}) \
                     after an error: {why}"
                ));
                return;
            }
        };
        self.rates = output.rates();
        self.device = output.name();
        if !self.rates.contains(&self.settings.rate) {
            self.settings.rate = output.sample_rate();
        }
        self.output = output;
        let note = format!(
            "the audio device is now {}, after an error: {why}",
            self.device
        );
        self.events.push(note.clone());
        self.note = Some(note);
        self.start_first(hosted);
        if self.is_processing() {
            self.events.push(format!("activated at {}", self.shape()));
        }
    }

    /// Says why processing is stopped, in the note and the log.
    fn stopped(&mut self, note: String) {
        self.events.push(note.clone());
        self.note = Some(note);
    }

    /// Returns the device, for a test that changes it.
    #[cfg(test)]
    pub(crate) fn output(&self) -> &D {
        &self.output
    }

    /// Ends the device's blocks: none runs once this returns.
    pub(crate) fn silence(&mut self) {
        drop(self.stream.take());
    }

    /// Returns what changed since the last call, in words, for the Activity
    /// log.
    pub(crate) fn events(&mut self) -> Vec<String> {
        std::mem::take(&mut self.events)
    }

    /// Returns the sample rate and the most frames a block holds, in words.
    pub(crate) fn shape(&self) -> String {
        format!("{}, {} frames", khz(self.settings.rate), self.settings.max)
    }

    /// Returns what the settings window shows of it.
    pub(crate) fn shown(&self) -> AudioShown<'_> {
        AudioShown {
            device: &self.device,
            device_rate: self.output.sample_rate(),
            channels: self.output.channels(),
            rates: &self.rates,
            settings: self.settings,
            signal: self.plan.signal(),
            processing: self.is_processing(),
            played: self.plan.played(),
            recent: self.plan.recent(),
            note: self.note.as_deref(),
        }
    }
}

/// A sample rate as a person reads it: `44.1 kHz`, `48 kHz`.
pub(crate) fn khz(rate: u32) -> String {
    if rate.is_multiple_of(1000) {
        format!("{} kHz", rate / 1000)
    } else {
        format!("{:.1} kHz", f64::from(rate) / 1000.0)
    }
}
