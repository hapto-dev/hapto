//! The standalone app: the plugin through the standalone exporter, its
//! editor in a window of the app's, and the test signal through it to the
//! audio device.

#[cfg(test)]
mod tests;

use std::error::Error as _;
use std::io::Read;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use hapto_host::standalone::{Standalone, StandaloneEditor};
use hapto_host::{Plugin, Size};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use crate::block::{Plan, Player};
use crate::device::{Output, Playing};
use crate::options::Options;
use crate::session::Session;
use crate::settings::Blocks;

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
}

/// Runs `P` standalone until its window closes.
///
/// # Errors
///
/// Returns why it could not run: no audio device, an activation refused, no
/// window.
pub(crate) fn run<P: Plugin>(options: &Options) -> Result<ExitCode, String> {
    let event_loop = EventLoop::<Wake>::with_user_event()
        .build()
        .map_err(|e| format!("the window's event loop: {e}"))?;
    if options.dev.is_some() {
        quit_when_stdin_closes(event_loop.create_proxy());
    }
    let mut session = Session::open(P::INFO.id, options.dev.as_deref())?;
    let plugin = Standalone::<P>::new();
    load_saved(&mut session, &plugin);
    let output = Output::system_default()?;
    let (rate, layout) = (output.sample_rate(), first_layout::<P>());
    plugin
        .activate(f64::from(rate), BLOCK, layout)
        .map_err(|e| format!("{}: {e}", P::INFO.name))?;
    let player = Player::of_app(
        &plugin,
        layout,
        output.channels(),
        f64::from(rate),
        BLOCK,
        Plan::new(Blocks::Device, options.signal),
    );
    let stream = output.start(rate, player)?;
    let mut app = App {
        stream: Some(stream),
        editor: None,
        window: None,
        failed: None,
        dev: options.dev.is_some(),
        next_tick: Instant::now(),
        next_keep: Instant::now() + KEEP,
        session,
        plugin,
    };
    let ran = event_loop.run_app(&mut app);
    app.finish();
    ran.map_err(|e| format!("the window's event loop: {e}"))?;
    match app.failed {
        Some(reason) => Err(reason),
        None => Ok(ExitCode::SUCCESS),
    }
}

/// Loads the session's saved block into `plugin`. A block the plugin
/// refuses, or one that cannot be read, is held: the session keeps it as it
/// is, rather than the plugin's defaults, for a build that loads it.
fn load_saved<P: Plugin>(session: &mut Session, plugin: &Standalone<P>) {
    match session.saved_block() {
        Ok(None) => {}
        Ok(Some(block)) => {
            if let Err(refused) = plugin.load(&block) {
                session.hold();
                let causes: String =
                    std::iter::successors(refused.source(), |&cause| cause.source())
                        .map(|cause| format!(": {cause}"))
                        .collect();
                eprintln!(
                    "{}: the saved block did not load ({refused}{causes}); it is kept for the \
                     next run",
                    P::INFO.name
                );
            }
        }
        Err(error) => eprintln!(
            "{}: the saved block could not be read ({error}); it is kept as it is",
            P::INFO.name
        ),
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

/// The title of the window the plugin `name` runs in, as it shows `editor`.
pub(crate) fn title(name: &str, editor: EditorShown) -> String {
    match editor {
        EditorShown::Open => name.to_owned(),
        EditorShown::None => format!("{name} (no editor)"),
        EditorShown::Failed => format!("{name} (its editor did not open)"),
    }
}

/// Whether a window's new `size` is a minimized one's, which winit gives on
/// Windows as 0 by 0: no size to hand an editor, as no host hands one. The
/// size it is restored to comes next.
pub(crate) fn minimized(size: PhysicalSize<u32>) -> bool {
    size.width == 0 || size.height == 0
}

/// The layout the app plays: the plugin's first.
pub(crate) fn first_layout<P: Plugin>() -> hapto_host::Layout {
    P::INFO
        .layouts
        .first()
        .copied()
        .unwrap_or(hapto_host::Layout::STEREO)
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

/// The app's state while its window is open. The fields drop in the order a
/// teardown must take: the device first, so no audio side outlives the
/// plugin, then the editor, then the window it is in, then the plugin.
struct App<P: Plugin> {
    stream: Option<Playing>,
    editor: Option<StandaloneEditor<P>>,
    window: Option<Window>,
    /// Why the app could not go on, once its loop ends.
    failed: Option<String>,
    dev: bool,
    next_tick: Instant,
    next_keep: Instant,
    session: Session,
    plugin: Standalone<P>,
}

impl<P: Plugin> App<P> {
    fn open_window(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let editor = self.plugin.editor();
        let size = editor.as_ref().map_or(NO_EDITOR, StandaloneEditor::size);
        let shown = match editor {
            Some(_) => EditorShown::Open,
            None => EditorShown::None,
        };
        let attributes = Window::default_attributes()
            .with_title(title(P::INFO.name, shown))
            .with_inner_size(LogicalSize::new(size.width, size.height));
        let window = event_loop
            .create_window(attributes)
            .map_err(|e| format!("the window: {e}"))?;
        // An editor that did not open goes, so nothing takes it for an open
        // one, and the title says so.
        let editor = editor.filter(|editor| {
            editor.set_scale(window.scale_factor());
            let Err(error) = editor.open(&window) else {
                return true;
            };
            eprintln!("{}: {error}", P::INFO.name);
            window.set_title(&title(P::INFO.name, EditorShown::Failed));
            false
        });
        self.editor = editor;
        self.window = Some(window);
        Ok(())
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
            eprintln!("the saved block: {error}");
        }
    }

    /// Says so once the device stopped playing on its own, an unplugged one
    /// say: in a line and in the window's title. The plugin stays as it is,
    /// its blocks over, until the app starts again.
    fn check_audio(&mut self) {
        let Some(why) = self.stream.as_ref().and_then(Playing::ended) else {
            return;
        };
        eprintln!("{}: the audio output stopped: {why}", P::INFO.name);
        drop(self.stream.take());
        if let Some(window) = &self.window {
            window.set_title(&format!("{} (audio stopped)", P::INFO.name));
        }
    }

    /// Keeps the saved block and tears down in order.
    fn finish(&mut self) {
        self.keep();
        drop(self.stream.take());
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
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        if now >= self.next_tick {
            self.plugin.tick();
            self.check_audio();
            self.next_tick = now + TICK;
        }
        if self.dev && now >= self.next_keep {
            self.keep();
            self.next_keep = now + KEEP;
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_tick));
    }
}
