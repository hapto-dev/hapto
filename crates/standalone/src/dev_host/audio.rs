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
//! A look once a second asks which device is the system's default output,
//! and follows it as a host follows a change of device. Processing on
//! another device, the plugin is stopped, then started on the new one. With
//! no default left, or after an error from the device that it does not play
//! on after, the plugin is stopped and waits, and each look takes the
//! default and tries the start again. Stopped by the author, a look takes a
//! new default and starts nothing.

#[cfg(test)]
mod tests;

use std::sync::Arc;

use anyhow::Result;
use cpal::DeviceId;
use hapto_host::layout::Width;

use super::hosted::Hosted;
use super::message::AudioShown;
use crate::block::{Plan, Player, Render};
use crate::device::{
    Answer, AudioSystem, DeviceError, Look, Output, Playing, answer, look, say_once,
};
use crate::settings::{MAX_BLOCKS, Settings};
use crate::signal::Signal;

/// An audio output that the dev host plays the plugin to, which a [`System`]
/// opens.
pub(crate) trait Device {
    /// What plays the blocks, until it drops.
    type Stream;
    /// What tells one device from another.
    type Id: PartialEq;
    /// Returns its name, as the system shows it.
    fn name(&self) -> String;
    /// Returns its id.
    fn id(&self) -> &Self::Id;
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
}

/// An audio system that the dev host finds the default output in: the
/// computer's, or a test's.
pub(crate) trait System {
    /// The outputs that it opens.
    type Output: Device;
    /// Returns the id of the default output now, `None` when there is none.
    fn default_output_id(&self) -> Option<<Self::Output as Device>::Id>;
    /// Opens the default output now, which may be another device than the
    /// one opened before.
    ///
    /// # Errors
    ///
    /// Returns why there is none to play to.
    fn open_default_output(&self) -> Result<Self::Output>;
}

impl Device for Output {
    type Stream = Playing;
    type Id = DeviceId;

    fn name(&self) -> String {
        Output::name(self)
    }

    fn id(&self) -> &DeviceId {
        Output::id(self)
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
}

impl System for AudioSystem {
    type Output = Output;

    fn default_output_id(&self) -> Option<DeviceId> {
        AudioSystem::default_output_id(self)
    }

    fn open_default_output(&self) -> Result<Output> {
        AudioSystem::open_default_output(self)
    }
}

/// The device, the audio system it came from, the settings the plugin runs
/// with, and the stream playing it.
pub(crate) struct Audio<S: System = AudioSystem> {
    output: S::Output,
    device: String,
    /// The sample rates the device takes, which alone are offered.
    rates: Vec<u32>,
    layout: Width,
    plan: Arc<Plan>,
    settings: Settings,
    /// The settings the plugin last started with, which a start it refuses
    /// brings back.
    ran: Option<Settings>,
    /// The stream playing the plugin's blocks, while it processes.
    stream: Option<<S::Output as Device>::Stream>,
    /// Why processing is stopped, or what the last change refused; cleared
    /// when the plugin processes again.
    note: Option<String>,
    /// What changed since the last look, for the Activity log.
    events: Vec<String>,
    /// Whether a change of device left the plugin waiting: each look tries
    /// the start again, until it processes or the author stops, starts or
    /// reactivates it.
    waits: bool,
    /// The audio system that a look asks for its default output, last so
    /// that it drops after the stream.
    system: S,
}

impl<S: System> Audio<S> {
    /// Opens `system`'s default output for a plugin laid out as `layout`,
    /// with `signal` on its inputs and `kept`, the settings a session kept,
    /// where the device still takes them, and else its own rate.
    ///
    /// # Errors
    ///
    /// Returns why there is no device to play to.
    pub(crate) fn open(
        system: S,
        layout: Width,
        signal: Signal,
        kept: Option<&str>,
    ) -> Result<Self> {
        let output = system.open_default_output()?;
        let rates = output.rates();
        let settings = kept
            .and_then(|text| Settings::read(text, &rates))
            .unwrap_or_else(|| Settings::starting(output.sample_rate()));
        Ok(Audio {
            device: output.name(),
            plan: Plan::new(settings.blocks, signal),
            output,
            system,
            rates,
            layout,
            settings,
            ran: None,
            stream: None,
            note: None,
            events: Vec::new(),
            waits: false,
        })
    }

    /// Returns the settings the plugin runs with.
    pub(crate) fn settings(&self) -> Settings {
        self.settings
    }

    /// Whether the plugin processes: not after [`stop`](Self::stop), nor
    /// after a start it refused, nor while a change of device stopped it.
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
    /// ones that last ran. A plugin a change of device left waiting waits no
    /// more.
    pub(crate) fn reactivate(&mut self, hosted: &dyn Hosted) {
        self.waits = false;
        let before = self.ran.filter(|ran| *ran != self.settings);
        self.restart(hosted, before);
    }

    /// Stops processing and deactivates the plugin, with no block running,
    /// until [`resume`](Self::resume). A plugin a change of device left
    /// waiting waits no more.
    pub(crate) fn stop(&mut self, hosted: &dyn Hosted) {
        if !self.is_processing() && !self.waits {
            return;
        }
        self.waits = false;
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

    /// Reads the device's newest error. One it plays on after goes in the
    /// log; any other [stops](Self::halt) the plugin at once, and the next
    /// [look](Self::look) starts it again on the default output.
    pub(crate) fn check(&mut self, hosted: &dyn Hosted) {
        let Some(error) = self.stream.as_mut().and_then(S::Output::error) else {
            return;
        };
        match answer(error.kind) {
            Answer::PlaysOn => self.events.push(format!(
                "the audio device reported an error and plays on: {}",
                error.text
            )),
            Answer::Reopen => self.halt(hosted, &error.text),
        }
    }

    /// Looks at which device is the system's default output, as a host
    /// follows a change of device. Returns whether the look changed the
    /// settings the plugin processes with, for the session to keep.
    ///
    /// Processing, the plugin moves to a new default, and with none left it
    /// stops and waits. Waiting, it takes the default and tries the start
    /// again. Stopped otherwise, by the author or a refusal, it takes a new
    /// default, so the Audio tab and its rates are true, and starts nothing.
    pub(crate) fn look(&mut self, hosted: &dyn Hosted) -> bool {
        let before = self.running();
        let default = self.system.default_output_id();
        if self.is_processing() || self.waits {
            let playing = self.is_processing().then(|| self.output.id());
            match look(playing, default.as_ref()) {
                Look::Keep | Look::Wait => {}
                Look::Move => self.move_on(hosted),
                Look::Stop => self.halt(hosted, "there is no audio output device"),
                Look::Open => self.start_again(hosted),
            }
        } else if default.is_some_and(|id| id != *self.output.id()) {
            // Tried again at the next look when the new default does not
            // open.
            let _adopted = self.adopt();
        }
        self.running() != before
    }

    /// Stops the plugin for a new default output, takes the device and
    /// starts the plugin there, as at the run's first start. A start that
    /// fails leaves the plugin waiting for the next look.
    fn move_on(&mut self, hosted: &dyn Hosted) {
        self.silence();
        hosted.stop();
        self.note = None;
        self.waits = true;
        if let Err(error) = self.adopt() {
            self.stopped(format!(
                "processing is stopped: the default output did not open ({error:#})"
            ));
            return;
        }
        self.start_first(hosted);
        if self.is_processing() {
            self.waits = false;
            self.events.push(format!("activated at {}", self.shape()));
        }
    }

    /// Takes the system's default output and starts the plugin there again,
    /// for a plugin a change of device left waiting. A failure is noted and
    /// logged once, and again only when its reason changes.
    fn start_again(&mut self, hosted: &dyn Hosted) {
        if let Err(error) = self.adopt() {
            self.stopped(format!(
                "processing is stopped: the default output did not open ({error:#})"
            ));
            return;
        }
        match self.start(hosted) {
            Ok(()) => {
                self.waits = false;
                self.note = None;
                self.events.push(format!("activated at {}", self.shape()));
            }
            Err(error) => self.stopped(format!("{error:#}: processing is stopped")),
        }
    }

    /// Stops processing and the plugin for a change of device, and leaves
    /// the plugin waiting for the next look; the note and the log say `why`.
    fn halt(&mut self, hosted: &dyn Hosted, why: &str) {
        self.silence();
        hosted.stop();
        self.waits = true;
        self.stopped(format!("processing is stopped: {why}"));
    }

    /// Takes the system's default output, as it is now: its name and its
    /// rates, and the settings where it offers their rate, else its own
    /// rate. A device other than the one before goes in the log.
    ///
    /// # Errors
    ///
    /// Returns why there is no default output to take.
    fn adopt(&mut self) -> Result<()> {
        let output = self.system.open_default_output()?;
        let moved = output.id() != self.output.id();
        self.rates = output.rates();
        self.device = output.name();
        if !self.rates.contains(&self.settings.rate) {
            self.settings.rate = output.sample_rate();
        }
        self.output = output;
        if moved {
            self.events
                .push(format!("the audio device is now {}", self.device));
        }
        Ok(())
    }

    /// Says why processing is stopped, in the note and the log, once, and
    /// again only when the reason changes.
    fn stopped(&mut self, note: String) {
        if let Some(note) = say_once(&mut self.note, note) {
            self.events.push(note);
        }
    }

    /// Returns the device, for a test that changes it.
    #[cfg(test)]
    pub(crate) fn output(&self) -> &S::Output {
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
