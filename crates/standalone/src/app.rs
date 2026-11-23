//! The standalone app: the plugin through the standalone exporter, its
//! editor in a window of the app's, and the test signal through it to the
//! audio device.

#[cfg(test)]
mod tests;

use std::error::Error as _;
use std::io::Read;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use hapto_host::layout::Width;
use hapto_host::standalone::{Standalone, StandaloneEditor};
use hapto_host::{Plugin, Size};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use crate::block::{Plan, Player};
use crate::device::{self, Answer, LOOK, Look, Output, Playing, answer, say_once};
use crate::options::Options;
use crate::session::Session;
use crate::settings::Blocks;
use crate::signal::Signal;

/// The most frames a block holds in the standalone app, which the plugin is
/// activated for: the device's buffers are cut at it.
pub(crate) const BLOCK: usize = 1024;

/// How often the plugin ticks: about 60 times a second, as an exporter's
/// timer ticks it.
pub(crate) const TICK: Duration = Duration::from_millis(16);

/// How often a dev session keeps the saved block when it changed, so a run
/// that ends without closing its window loses at most this much.
pub(crate) const KEEP: Duration = Duration::from_secs(1);

/// The window's size for a plugin with no editor.
pub(crate) const NO_EDITOR: Size = Size {
    width: 420,
    height: 160,
};

/// What wakes the event loop from another thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wake {
    /// `cargo hapto dev` closed the app's stdin: save and quit.
    Quit,
    /// A tick is due. The loop's own deadline is not served while the system
    /// runs a loop of its own, a held title bar's or an open window menu's,
    /// and an event from another thread is.
    Tick,
}

/// Runs `P` standalone until its window closes.
///
/// # Errors
///
/// Returns why it could not run: no audio device, an activation refused, no
/// window.
pub(crate) fn run<P: Plugin>(options: &Options) -> Result<ExitCode> {
    let event_loop = EventLoop::<Wake>::with_user_event()
        .build()
        .context("the window's event loop")?;
    if options.dev.is_some() {
        quit_when_stdin_closes(event_loop.create_proxy());
    }
    tick_from_a_thread(event_loop.create_proxy());
    let mut session = Session::open(P::INFO.id, options.dev.as_deref())?;
    let plugin = Standalone::<P>::new();
    if let Some(line) = load_saved(&mut session, &plugin) {
        eprintln!("{line}");
    }
    if let Some(line) = silence_note(P::INFO.name, hapto_host::first_width::<P>(), options.signal) {
        eprintln!("{line}");
    }
    let audio = start_audio(&plugin, options.signal)?;
    let mut app = App {
        audio: Some(audio),
        editor: None,
        window: None,
        failed: None,
        dev: options.dev.is_some(),
        signal: options.signal,
        failing: None,
        shows: TitleShows {
            editor: EditorShown::None,
            audio_stopped: false,
            faults: 0,
        },
        next_tick: Instant::now(),
        next_look: Instant::now() + LOOK,
        next_keep: Instant::now() + KEEP,
        session,
        plugin,
    };
    let ran = event_loop.run_app(&mut app);
    app.finish();
    ran.context("the window's event loop")?;
    match app.failed {
        Some(reason) => Err(reason),
        None => Ok(ExitCode::SUCCESS),
    }
}

/// Opens the system's default output, activates `plugin` at its rate in its
/// first layout, and starts the output playing it, with `signal` on its
/// inputs. A device that does not start leaves the plugin deactivated again.
///
/// # Errors
///
/// Returns why it did not start: no device, an activation refused, or a
/// device that did not start.
fn start_audio<P: Plugin>(plugin: &Standalone<P>, signal: Signal) -> Result<Audio> {
    let output = Output::system_default()?;
    let (rate, layout) = (output.sample_rate(), hapto_host::first_width::<P>());
    plugin
        .activate(f64::from(rate), BLOCK, layout)
        .context(P::INFO.name)?;
    let player = Player::of_app(
        plugin,
        layout,
        output.channels(),
        f64::from(rate),
        BLOCK,
        Plan::new(Blocks::Device, signal),
    );
    let stream = output
        .start(rate, player)
        .inspect_err(|_| plugin.deactivate())?;
    Ok(Audio { stream, output })
}

/// The audio playing the plugin: the stream, then the device it plays on.
struct Audio {
    stream: Playing,
    output: Output,
}

impl Audio {
    /// Returns where it plays, in words: the device's name and rate.
    fn place(&self) -> String {
        format!("{} at {} Hz", self.output.name(), self.output.sample_rate())
    }
}

/// Returns the line that tells the author of the plugin `name`, laid out as
/// `layout`, that its inputs hear silence, when `signal` is silence: the
/// first run of an effect is silent otherwise, with no word of why. An
/// instrument, with no inputs, gets none.
pub(crate) fn silence_note(name: &str, layout: Width, signal: Signal) -> Option<String> {
    (signal == Signal::Silence && layout.inputs > 0)
        .then(|| format!("{name}: its inputs hear silence; --signal sine plays a tone into them"))
}

/// Loads the session's saved block into `plugin`. A block the plugin
/// refuses, or one that cannot be read, is held: the session keeps it as it
/// is, rather than the plugin's defaults, for a build that loads it. Returns
/// the line that says so, and where the block is.
fn load_saved<P: Plugin>(session: &mut Session, plugin: &Standalone<P>) -> Option<String> {
    let name = P::INFO.name;
    match session.saved_block() {
        Ok(None) => None,
        Ok(Some(block)) => {
            let refused = plugin.load(&block).err()?;
            session.hold();
            let causes: String = std::iter::successors(refused.source(), |&cause| cause.source())
                .map(|cause| format!(": {cause}"))
                .collect();
            let held = session.held_note();
            Some(format!(
                "{name}: the saved block did not load ({refused}{causes}); {held}"
            ))
        }
        Err(error) => Some(format!(
            "{name}: the saved block could not be read ({error:#}); {}",
            session.held_note()
        )),
    }
}

/// What the window shows of the plugin's editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditorShown {
    /// The editor, open in it.
    Open,
    /// Nothing: the plugin has no editor, or the format shows none.
    None,
    /// Nothing: the plugin's editor did not open.
    Failed,
}

/// What the window's title says beside the plugin's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TitleShows {
    /// What the window shows of the editor.
    pub(crate) editor: EditorShown,
    /// Whether the audio is stopped: no device plays the plugin.
    pub(crate) audio_stopped: bool,
    /// How many faults the plugin has had, each kind's first written to the
    /// terminal.
    pub(crate) faults: u64,
}

/// The title of the window the plugin `name` runs in, as `shows` says.
pub(crate) fn title(name: &str, shows: TitleShows) -> String {
    let mut notes = Vec::new();
    match shows.editor {
        EditorShown::Open => {}
        EditorShown::None => notes.push("no editor".to_owned()),
        EditorShown::Failed => notes.push("its editor did not open".to_owned()),
    }
    if shows.audio_stopped {
        notes.push("audio stopped".to_owned());
    }
    match shows.faults {
        0 => {}
        1 => notes.push("1 fault, see the terminal".to_owned()),
        faults => notes.push(format!("{faults} faults, see the terminal")),
    }
    if notes.is_empty() {
        name.to_owned()
    } else {
        format!("{name} ({})", notes.join("; "))
    }
}

/// Whether a window's new `size` is a minimized one's, which winit gives on
/// Windows as 0 by 0: no size to hand an editor, as no host hands one. The
/// size it is restored to comes next.
pub(crate) fn minimized(size: PhysicalSize<u32>) -> bool {
    size.width == 0 || size.height == 0
}

/// Wakes the event loop to quit once stdin closes, as `cargo hapto dev` does
/// to restart the app after a rebuild.
pub(crate) fn quit_when_stdin_closes(proxy: EventLoopProxy<Wake>) {
    std::thread::spawn(move || {
        let mut sink = [0_u8; 64];
        let mut stdin = std::io::stdin();
        while matches!(stdin.read(&mut sink), Ok(read) if read > 0) {}
        let _closed = proxy.send_event(Wake::Quit);
    });
}

/// Wakes the event loop every [`TICK`] from a thread of its own, until the
/// loop is gone.
pub(crate) fn tick_from_a_thread(proxy: EventLoopProxy<Wake>) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(TICK);
            if proxy.send_event(Wake::Tick).is_err() {
                break;
            }
        }
    });
}

/// The app's state while its window is open. The fields drop in the order a
/// teardown must take: the device first, so no audio side outlives the
/// plugin, then the editor, then the window it is in, then the plugin.
struct App<P: Plugin> {
    /// The audio, while a device plays the plugin.
    audio: Option<Audio>,
    editor: Option<StandaloneEditor<P>>,
    window: Option<Window>,
    /// Why the app could not go on, once its loop ends.
    failed: Option<anyhow::Error>,
    dev: bool,
    /// What plays into the plugin's inputs, kept for the next device.
    signal: Signal,
    /// Why the default output last did not open, said once until the audio
    /// plays again.
    failing: Option<String>,
    shows: TitleShows,
    next_tick: Instant,
    next_look: Instant,
    next_keep: Instant,
    session: Session,
    plugin: Standalone<P>,
}

impl<P: Plugin> App<P> {
    fn open_window(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let editor = self.plugin.editor();
        let size = editor.as_ref().map_or(NO_EDITOR, StandaloneEditor::size);
        self.shows.editor = match editor {
            Some(_) => EditorShown::Open,
            None => EditorShown::None,
        };
        let attributes = Window::default_attributes()
            .with_title(title(P::INFO.name, self.shows))
            .with_inner_size(LogicalSize::new(size.width, size.height));
        let window = event_loop.create_window(attributes).context("the window")?;
        // An editor that did not open goes, so nothing takes it for an open
        // one, and the title says so.
        let editor = editor.filter(|editor| {
            editor.set_scale(window.scale_factor());
            let Err(error) = editor.open(&window) else {
                return true;
            };
            eprintln!("{}: {error}", P::INFO.name);
            false
        });
        if editor.is_none() && self.shows.editor == EditorShown::Open {
            self.shows.editor = EditorShown::Failed;
        }
        self.editor = editor;
        self.window = Some(window);
        self.retitle();
        Ok(())
    }

    /// Sets the window's title to what it shows now.
    fn retitle(&self) {
        if let Some(window) = &self.window {
            window.set_title(&title(P::INFO.name, self.shows));
        }
    }

    fn resized(&self, size: PhysicalSize<u32>) {
        if minimized(size) {
            return;
        }
        if let (Some(editor), Some(window)) = (&self.editor, &self.window) {
            let logical: LogicalSize<u32> = size.to_logical(window.scale_factor());
            editor.set_size(Size {
                width: logical.width,
                height: logical.height,
            });
        }
    }

    fn keep(&mut self) {
        if let Err(error) = self.session.keep(self.plugin.save()) {
            eprintln!("the saved block: {error:#}");
        }
    }

    /// Reads the device's newest error. One it plays on after is said in a
    /// line; any other [stops the audio](Self::stop_audio), and the next
    /// [look](Self::look) opens the default output again.
    fn check_audio(&mut self) {
        let Some(error) = self.audio.as_mut().and_then(|audio| audio.stream.error()) else {
            return;
        };
        match answer(error.kind) {
            Answer::PlaysOn => eprintln!(
                "{}: the audio output reported an error and plays on: {}",
                P::INFO.name,
                error.text
            ),
            Answer::Reopen => self.stop_audio(&error.text),
        }
    }

    /// Looks at which device is the system's default output, and follows
    /// it: to a new default, with a line that says so; to none, which stops
    /// the audio; and, while nothing plays, by opening the default.
    fn look(&mut self) {
        let name = P::INFO.name;
        let default = device::default_output_id();
        let playing = self.audio.as_ref().map(|audio| audio.output.id());
        match device::look(playing, default.as_ref()) {
            Look::Keep | Look::Wait => {}
            Look::Move => {
                self.silence();
                if let Some(place) = self.open_audio() {
                    eprintln!(
                        "{name}: the audio output now plays on {place}: the default output changed"
                    );
                }
            }
            Look::Stop => self.stop_audio("there is no audio output device"),
            Look::Open => {
                if let Some(place) = self.open_audio() {
                    eprintln!("{name}: the audio output plays again, on {place}");
                }
            }
        }
    }

    /// Opens the system's default output and plays the plugin there, as a
    /// host does for a new device, and returns where it plays. One that does
    /// not open leaves the audio stopped, which the window's title says, and
    /// a line says why, once, and again only when the reason changes.
    fn open_audio(&mut self) -> Option<String> {
        match start_audio(&self.plugin, self.signal) {
            Ok(audio) => {
                let place = audio.place();
                self.audio = Some(audio);
                self.failing = None;
                self.show_audio_stopped(false);
                Some(place)
            }
            Err(error) => {
                let reason = format!("the default output did not open ({error:#})");
                if let Some(reason) = say_once(&mut self.failing, reason) {
                    eprintln!("{}: the audio output stopped: {reason}", P::INFO.name);
                }
                self.show_audio_stopped(true);
                None
            }
        }
    }

    /// Stops the audio, which a line with `why` and the window's title say.
    fn stop_audio(&mut self, why: &str) {
        self.silence();
        eprintln!("{}: the audio output stopped: {why}", P::INFO.name);
        self.show_audio_stopped(true);
    }

    /// Drops the stream, so its blocks end before the plugin is touched,
    /// then deactivates the plugin: it is inactive whenever no device plays
    /// it.
    fn silence(&mut self) {
        drop(self.audio.take());
        self.plugin.deactivate();
    }

    /// Says in the window's title whether the audio is `stopped`.
    fn show_audio_stopped(&mut self, stopped: bool) {
        if self.shows.audio_stopped != stopped {
            self.shows.audio_stopped = stopped;
            self.retitle();
        }
    }

    /// Ticks the plugin, reads the device and the faults, looks at which
    /// device is the default output, and keeps the saved block, each when it
    /// is due. A fault's line goes to the terminal, and the title counts
    /// them.
    fn periodic(&mut self) {
        let now = Instant::now();
        if now >= self.next_tick {
            self.plugin.tick();
            self.check_audio();
            let faults = self.plugin.faults();
            if faults != self.shows.faults {
                self.shows.faults = faults;
                self.retitle();
            }
            self.next_tick = now + TICK;
        }
        if now >= self.next_look {
            self.look();
            // From the look's end, as `LOOK` says.
            self.next_look = Instant::now() + LOOK;
        }
        if self.dev && now >= self.next_keep {
            self.keep();
            self.next_keep = now + KEEP;
        }
    }

    /// Keeps the saved block and tears down in order.
    fn finish(&mut self) {
        self.keep();
        drop(self.audio.take());
        drop(self.editor.take());
        drop(self.window.take());
    }
}

impl<P: Plugin> ApplicationHandler<Wake> for App<P> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none()
            && let Err(error) = self.open_window(event_loop)
        {
            self.failed = Some(error);
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.resized(size),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(editor) = &self.editor {
                    editor.set_scale(scale_factor);
                }
            }
            _ => {}
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Wake) {
        match event {
            Wake::Quit => event_loop.exit(),
            Wake::Tick => self.periodic(),
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.periodic();
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_tick));
    }
}
