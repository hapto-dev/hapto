//! The dev host, `--host`: the plugin through a real plugin format, in
//! process, by Hapto's own test hosts.
//!
//! Its window is the plugin's, as a host's would be, with a bar across the
//! top: the format the plugin runs through, the audio, the faults, and a
//! button for the settings window. In VST3 the plugin goes through the
//! exporter's COM interfaces, as a host calls them, ticked by the exporter's
//! own timer on the window's message loop, and its editor is the exporter's
//! view, below the bar. In the C interface it goes through the table the
//! JUCE adapter drives, ticked by the dev host, with no editor.
//!
//! The settings window shows what the host sees, a tab each: the audio,
//! changed as a host changes it ([`audio`]); the parameters as the host
//! lists them, with a control for each; the Activity log of what the plugin
//! told the host and what the dev host did ([`activity`]); the saved block
//! and how its first load went ([`saved`]); and the faults. A switch of
//! format keeps the saved block and the format in the session and starts
//! the app again: under `cargo hapto dev` by its exit code, which asks the
//! command to, and otherwise by starting itself.
//!
//! Open: the plugin is linked in, so neither the module's loading nor its
//! factory is the one a host meets; and the instance is stopped and
//! deactivated at the close but never released, so a run's teardown is not
//! the one a host meets either.

mod activity;
mod audio;
mod format;
mod hosted;
mod layout;
mod learned;
mod message;
mod panel;
mod pending;
mod saved;
mod windows;

use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, Result};
use hapto_host::Plugin;
use hapto_host::vst3::{ChildWindow, TestView};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::app::{
    EditorShown, KEEP, TICK, Wake, first_layout, quit_when_stdin_closes, silence_note,
    tick_from_a_thread,
};
use crate::device::LOOK;
use crate::options::Options;
use crate::session::{RELAUNCH, Session};
use activity::Activity;
use audio::Audio;
pub use format::Format;
pub(crate) use hosted::{Hosted, host};
use message::{About, BarShown, Message, Shown};
use panel::Panel;
use saved::{FirstLoad, LastSave};

/// How often the bar and the settings window show what changed.
const SHOW: Duration = Duration::from_millis(100);

/// Runs `P` in the dev host until its window closes, or its format is
/// switched.
///
/// # Errors
///
/// Returns why it could not run: no audio device, a call the plugin
/// refused, no window.
pub(crate) fn run<P: Plugin>(options: &Options) -> Result<ExitCode> {
    let event_loop = EventLoop::<Wake>::with_user_event()
        .build()
        .context("the window's event loop")?;
    if options.dev.is_some() {
        quit_when_stdin_closes(event_loop.create_proxy());
    }
    tick_from_a_thread(event_loop.create_proxy());
    let now = Instant::now();
    let mut session = Session::open(P::INFO.id, options.dev.as_deref())?;
    let format = session
        .format()
        .and_then(|name| Format::named(&name))
        .unwrap_or(Format::Vst3);
    let hosted = hosted::host::<P>(format);
    let first_load = FirstLoad::of(session.saved_block(), &*hosted);
    if first_load.holds() {
        session.hold();
        eprintln!(
            "{}: {}; {}",
            P::INFO.name,
            first_load.note(),
            session.held_note()
        );
    }
    if let Some(line) = silence_note(P::INFO.name, first_layout::<P>(), options.signal) {
        eprintln!("{line}");
    }
    let kept = session.audio();
    let mut audio = Audio::open(first_layout::<P>(), options.signal, kept.as_deref())?;
    audio.start_first(&*hosted);
    if let Some(running) = audio.running()
        && let Err(error) = session.set_audio(&running.write())
    {
        eprintln!("the audio settings: {error:#}");
    }
    let mut activity = Activity::new(now);
    activity.note(
        format!("started in {} at {}", format.label(), audio.shape()),
        now,
    );
    activity.note(first_load.note(), now);
    let mut app = DevHost {
        audio,
        settings: None,
        view: None,
        editor: EditorShown::None,
        frame: None,
        bar: None,
        window: None,
        hosted,
        session,
        about: About {
            name: P::INFO.name,
            id: P::INFO.id,
            version: P::INFO.version,
            midi_in: P::INFO.midi_input,
            midi_out: P::INFO.midi_output,
        },
        dev: options.dev.is_some(),
        next_tick: now,
        next_show: now,
        next_look: now + LOOK,
        next_keep: now + KEEP,
        started: since_1970(SystemTime::now()),
        first_load,
        last_save: LastSave::default(),
        activity,
        shown: Told::default(),
        switch: None,
        failed: None,
    };
    let ran = event_loop.run_app(&mut app);
    app.finish();
    ran.context("the window's event loop")?;
    if let Some(reason) = app.failed {
        return Err(reason);
    }
    match app.switch {
        Some(_) if options.dev.is_some() => Ok(ExitCode::from(RELAUNCH)),
        Some(_) => start_again(),
        None => Ok(ExitCode::SUCCESS),
    }
}

/// Starts this app again with the same command line, for a switch of format
/// with no `cargo hapto dev` to do it.
fn start_again() -> Result<ExitCode> {
    let exe = std::env::current_exe().context("the app's own path")?;
    std::process::Command::new(&exe)
        .args(std::env::args_os().skip(1))
        .spawn()
        .with_context(|| exe.display().to_string())?;
    Ok(ExitCode::SUCCESS)
}

/// Returns `time` in milliseconds since 1970, 0 before it.
fn since_1970(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The settings window, a page in a window of its own. The panel goes
/// before its window.
struct SettingsWindow {
    panel: Panel,
    window: Window,
    page: PageState,
}

/// How far the settings window's page has come. A script run before the
/// page has loaded finds no `update` to call, and wry keeps no script for
/// later, so the page says when it is ready.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PageState {
    /// Loading: it is told nothing yet.
    Loading,
    /// Loaded: the next update brings it everything.
    Ready,
    /// Told everything once: an update brings what changed.
    Told,
}

/// What the settings window was last told, so an update brings only what
/// changed.
#[derive(Default)]
struct Told {
    /// The last Activity entry it has.
    activity: u64,
    /// The saved block it shows.
    saved: Option<Vec<u8>>,
    /// What each parameter was learned from, in that block.
    learned: Vec<String>,
}

/// The dev host's state while its windows are open. A teardown goes in the
/// order of the fields: the device first, then the settings window, the
/// view, the window it is in, the bar, and the main window.
struct DevHost {
    audio: Audio,
    settings: Option<SettingsWindow>,
    view: Option<TestView>,
    /// What the window shows of the editor: whether a view that was made
    /// opened, which the bar's note says.
    editor: EditorShown,
    /// The window the view is attached to, below the bar.
    frame: Option<ChildWindow>,
    bar: Option<Panel>,
    window: Option<Window>,
    hosted: Box<dyn Hosted>,
    session: Session,
    about: About<'static>,
    dev: bool,
    next_tick: Instant,
    next_show: Instant,
    next_look: Instant,
    next_keep: Instant,
    /// When the run started, in milliseconds since 1970.
    started: u64,
    first_load: FirstLoad,
    last_save: LastSave,
    activity: Activity,
    shown: Told,
    /// The format to start again in.
    switch: Option<Format>,
    failed: Option<anyhow::Error>,
}

impl DevHost {
    /// Ticks the plugin, looks at which device is the default output, keeps
    /// the saved block and shows what the host sees, each when it is due,
    /// and ends the loop for a switch of format. The block is kept before it
    /// is shown, so the two take the same save.
    fn periodic(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if now >= self.next_tick {
            self.tick(event_loop);
            self.next_tick = now + TICK;
        }
        if now >= self.next_look {
            self.look(now);
            // From the look's end, as `LOOK` says.
            self.next_look = Instant::now() + LOOK;
        }
        if self.dev && now >= self.next_keep {
            self.keep(now);
            self.next_keep = now + KEEP;
        }
        if now >= self.next_show {
            self.show(now);
            self.next_show = now + SHOW;
        }
        if self.switch.is_some() {
            event_loop.exit();
        }
    }

    fn tick(&mut self, event_loop: &ActiveEventLoop) {
        self.hosted.tick();
        let now = Instant::now();
        self.activity.calls(self.hosted.calls(), now);
        self.follow_the_view();
        let mut messages = self.bar.as_ref().map(Panel::messages).unwrap_or_default();
        if let Some(settings) = &self.settings {
            messages.extend(settings.panel.messages());
        }
        for message in messages {
            self.handle(message, event_loop);
        }
        self.audio.check(&*self.hosted);
        for event in self.audio.events() {
            self.activity.note(event, now);
        }
    }

    /// Follows the system's default output, and keeps the settings a look
    /// changed once the plugin processes with them, so the next run starts
    /// with them rather than drop them for a rate its device lacks.
    fn look(&mut self, now: Instant) {
        if self.audio.look(&*self.hosted) {
            self.keep_audio();
        }
        for event in self.audio.events() {
            self.activity.note(event, now);
        }
    }

    fn handle(&mut self, message: Message, event_loop: &ActiveEventLoop) {
        let hosted = &*self.hosted;
        match message {
            Message::Set(index, normalized) => hosted.set(index, normalized),
            Message::Switch(format) => self.switch = Some(format),
            Message::Audio(change) => {
                self.audio
                    .change(hosted, change.apply(self.audio.settings()));
                self.keep_audio();
            }
            Message::Signal(signal) => self.audio.set_signal(signal),
            Message::Reactivate => {
                self.audio.reactivate(hosted);
                self.keep_audio();
            }
            Message::Processing(true) => {
                self.audio.resume(hosted);
                self.keep_audio();
            }
            Message::Processing(false) => self.audio.stop(hosted),
            Message::Settings => {
                if let Err(error) = self.open_settings(event_loop) {
                    eprintln!("{}: {error:#}", self.about.name);
                }
            }
            Message::Ready => {
                if let Some(settings) = &mut self.settings {
                    settings.page = PageState::Ready;
                }
            }
        }
    }

    fn show(&mut self, now: Instant) {
        let faults = self.hosted.faults();
        if let Some(bar) = &self.bar {
            let shown = BarShown {
                format: self.hosted.format(),
                settings: self.audio.settings(),
                processing: self.audio.is_processing(),
                faults,
                editor: self.editor,
            };
            bar.run(&shown.script());
        }
        let Some(settings) = &self.settings else {
            return;
        };
        // A page still loading finds no update to take, and a hidden one
        // is told what changed once it shows again.
        if settings.page == PageState::Loading || settings.window.is_visible() == Some(false) {
            return;
        }
        let opened = settings.page == PageState::Ready;
        let told = &mut self.shown;
        // A page just opened takes a fresh block, and else the last one
        // saved, which is less than a second old.
        let hosted = &*self.hosted;
        let saved = if opened {
            self.last_save.fresh(now, || hosted.save())
        } else {
            self.last_save.recent(now, || hosted.save())
        };
        let changed = saved.is_some() && (opened || saved != told.saved);
        if changed {
            let block = saved.as_deref().unwrap_or_default();
            told.learned = learned::learned(block, self.hosted.params());
        }
        let after = if opened { 0 } else { told.activity };
        let shown = Shown {
            plugin: self.about,
            format: self.hosted.format(),
            params: self.hosted.params(),
            values: (0..self.hosted.params().len())
                .map(|index| self.hosted.value(index))
                .collect(),
            learned: changed.then_some(told.learned.as_slice()),
            activity: self.activity.since(after).collect(),
            faults,
            latency: self.hosted.latency(),
            saved: changed.then(|| block_text(saved.as_deref().unwrap_or_default())),
            first_load: opened.then_some(&self.first_load),
            started: self.started,
            audio: self.audio.shown(),
        };
        settings.panel.run(&shown.script());
        told.activity = self.activity.last();
        if changed {
            told.saved = saved;
        }
        if let Some(settings) = &mut self.settings {
            settings.page = PageState::Told;
        }
    }

    /// Keeps the audio settings for the next run, a rebuild's included: only
    /// the settings the plugin processes with, so a run never starts with
    /// ones it has not taken.
    fn keep_audio(&self) {
        let Some(running) = self.audio.running() else {
            return;
        };
        if let Err(error) = self.session.set_audio(&running.write()) {
            eprintln!("the audio settings: {error:#}");
        }
    }

    /// Keeps the plugin's saved block for the next run, saved at `now`,
    /// unless the session holds its own.
    fn keep(&mut self, now: Instant) {
        let hosted = &*self.hosted;
        let Some(block) = self.last_save.fresh(now, || hosted.save()) else {
            return;
        };
        if let Err(error) = self.session.keep(block) {
            eprintln!("the saved block: {error:#}");
        }
    }

    /// Keeps the saved block and the format to start in, and tears down in
    /// order: once the view is removed, the plugin is stopped and
    /// deactivated, as a host does before it releases one.
    fn finish(&mut self) {
        self.keep(Instant::now());
        if let Some(format) = self.switch
            && let Err(error) = self.session.set_format(format.name())
        {
            eprintln!("the format: {error:#}");
        }
        self.audio.silence();
        drop(self.settings.take());
        if let Some(view) = self.view.take() {
            let _removed = view.remove();
        }
        self.hosted.stop();
        drop(self.frame.take());
        drop(self.bar.take());
        drop(self.window.take());
    }
}

impl ApplicationHandler<Wake> for DevHost {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none()
            && let Err(error) = self.open_window(event_loop)
        {
            self.failed = Some(error);
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.is_settings(id) {
            self.settings_event(event);
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.resized(size),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(view) = &self.view {
                    let _scaled = view.set_scale(scale_factor as f32);
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Wake) {
        match event {
            Wake::Quit => event_loop.exit(),
            Wake::Tick => self.periodic(event_loop),
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.periodic(event_loop);
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_tick.min(self.next_show)));
    }
}

/// The saved block as text, for the page to lay out: it keeps the block's
/// own order of keys, which a parsed `serde_json::Value` sorts.
fn block_text(block: &[u8]) -> String {
    String::from_utf8_lossy(block).into_owned()
}
