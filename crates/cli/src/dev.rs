//! `cargo hapto dev`: a plugin running while its author edits it.
//!
//! - **Standalone**, the default: the crate's standalone binary, the plugin
//!   through Hapto's standalone exporter in a window of its own. A change to
//!   a member's Rust rebuilds it, and the running app is asked to quit,
//!   keeping its saved block, and started again with it.
//! - **`--host`**: the same binary, built with the dev host, which runs the
//!   plugin through the VST3 exporter or the C interface with a settings
//!   window, and restarts the same way.
//!
//! In each, the page comes from its dev server, so an edit to the page shows
//! at once, and the page's parameter map is written again before each build.
//! A real host takes the debug bundle instead, with its page from `cargo
//! hapto serve`.

#[cfg(test)]
mod tests;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use crate::args::Args;
use crate::bundle::Profile;
use crate::page::{self, DEV_PORT, DevServer, PAGE_DEV_URL, write_param_map};
use crate::tool::{self, cargo};
use crate::watch::Watch;
use crate::workspace::{Plugin, Workspace};

/// What `cargo hapto help` prints for this command.
pub(crate) const USAGE: &str =
    "  dev [-p <crate>] [--host] [--signal sine|noise|silence] [--port <n>] [--no-page]
      [--release]
                    run the plugin standalone, rebuilding and restarting it with its saved
                    block kept after each change to Rust, with the page from its dev server;
                    --host runs it through the dev host, the VST3 exporter or the C interface
                    with a settings window. --release builds the page into the app instead,
                    since a release build reads no dev server";

/// The exit code with which the app asks to be started again at once, as a
/// switch of the dev host's format does: `hapto-standalone`'s own, which a
/// test reads, since this command links no Hapto crate.
pub(crate) const RELAUNCH: i32 = 3;

/// How often the app and the sources are looked at.
const POLL: Duration = Duration::from_millis(400);

/// Where the app's page comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PageSource {
    /// No page: the crate has none, or `--no-page`.
    None,
    /// The dev server, whose address a debug build reads.
    Server,
    /// Built into the app, as a release build embeds it.
    Built,
}

/// Returns where the page comes from, for a crate that `has_page` and a
/// run that `wants` it, built in `profile`.
fn page_source(has_page: bool, wants: bool, profile: Profile) -> PageSource {
    match (has_page && wants, profile) {
        (false, _) => PageSource::None,
        (true, Profile::Debug) => PageSource::Server,
        (true, Profile::Release) => PageSource::Built,
    }
}

/// How the app ended, and what the dev loop does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ended {
    /// It asked with [`RELAUNCH`] to be started again at once.
    Relaunch,
    /// It closed, with exit code 0: the session ends.
    Closed,
    /// It failed or crashed, with this code, or none: the session goes on,
    /// and the next build that succeeds starts it again.
    Failed(Option<i32>),
}

impl Ended {
    /// Reads the app's exit `code`, `None` for an app ended with none.
    fn of(code: Option<i32>) -> Ended {
        match code {
            Some(RELAUNCH) => Ended::Relaunch,
            Some(0) => Ended::Closed,
            other => Ended::Failed(other),
        }
    }
}

/// What the dev loop finds of the app once a build ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seen {
    /// It runs the last build.
    Running,
    /// It ended while the build ran.
    Ended(Ended),
    /// None runs, since one crashed before the build began.
    Stopped,
}

/// What a build in the dev loop did to the app's binary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Build {
    /// It failed: the last binary is the one there is.
    Failed,
    /// Cargo found the binary up to date and wrote nothing.
    Unchanged,
    /// It wrote a new binary.
    New,
}

/// What the dev loop does once a build ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Next {
    /// End the session: the app was closed.
    End,
    /// Start the app: the new build, or the last one when the build failed
    /// and the app asked to be started again.
    Start,
    /// Leave the app running the binary it runs.
    Keep,
    /// Run nothing until a build succeeds.
    Wait,
}

/// What the dev loop does once a `build` ends, the app as `seen`. A close
/// while the build ran ends the session, as it does between builds, and a
/// build that wrote no new binary leaves a running app as it is.
fn after_build(seen: Seen, build: Build) -> Next {
    match (seen, build) {
        (Seen::Ended(Ended::Closed), _) => Next::End,
        (Seen::Ended(Ended::Relaunch), _) | (Seen::Running, Build::New) => Next::Start,
        (Seen::Running, Build::Unchanged | Build::Failed) => Next::Keep,
        (Seen::Ended(Ended::Failed(_)) | Seen::Stopped, Build::New | Build::Unchanged) => {
            Next::Start
        }
        (Seen::Ended(Ended::Failed(_)) | Seen::Stopped, Build::Failed) => Next::Wait,
    }
}

/// An app's exit code, as the loop's lines name it.
fn exit_code(code: Option<i32>) -> String {
    code.map_or("no exit code".to_owned(), |c| format!("exit code {c}"))
}

/// How long the app has to quit once asked, before it is ended.
const QUIT: Duration = Duration::from_secs(5);

/// What the plugin runs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// The standalone app.
    Standalone,
    /// The standalone app's dev host.
    DevHost,
}

/// What `cargo hapto dev` was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Options {
    /// The plugin crate, by its package name; `None` finds it.
    pub(crate) package: Option<String>,
    /// What the plugin runs in.
    pub(crate) mode: Mode,
    /// The app's test signal, by name.
    pub(crate) signal: Option<String>,
    /// The dev server's port.
    pub(crate) port: u16,
    /// Whether to serve the page from its dev server.
    pub(crate) page: bool,
    /// The build to run.
    pub(crate) profile: Profile,
}

impl Options {
    /// Reads the command's arguments, its own name excluded.
    ///
    /// # Errors
    ///
    /// Returns what is wrong with them.
    pub(crate) fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
        let mut args = Args::new(args, USAGE);
        let mut options = Options {
            package: None,
            mode: Mode::Standalone,
            signal: None,
            port: DEV_PORT,
            page: true,
            profile: Profile::Debug,
        };
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-p" | "--package" => options.package = Some(args.value(&arg)?),
                "--host" => options.mode = Mode::DevHost,
                "--signal" => options.signal = Some(args.value(&arg)?),
                "--port" => options.port = args.port(&arg)?,
                "--no-page" => options.page = false,
                "--release" => options.profile = Profile::Release,
                _ => return Err(args.unknown(&arg)),
            }
        }
        Ok(options)
    }
}

/// Runs `cargo hapto dev` with the arguments after the command's name, until
/// the plugin's window closes.
///
/// # Errors
///
/// Returns the step that failed and why.
pub(crate) fn run_command(args: impl IntoIterator<Item = String>) -> Result<(), String> {
    let options = Options::parse(args)?;
    let (workspace, plugin) = Workspace::find(options.package.as_deref())?;
    // A crate with no standalone binary is told so before the page's
    // packages and server, which take time.
    let session = workspace
        .target
        .join("hapto")
        .join("dev")
        .join(&plugin.package);
    let app = App::of(&plugin, &options, &session)?;
    let source = page_source(plugin.page().is_some(), options.page, options.profile);
    let server = match (source, plugin.page()) {
        (PageSource::Server, Some(page)) => Some(DevServer::start(&page, options.port)?),
        (PageSource::Built, Some(page)) => {
            page::prepare(&workspace, &plugin, &page)?;
            println!(
                "a release build embeds the page, built now: a change to the page shows \
                 once cargo hapto dev starts again"
            );
            None
        }
        _ => None,
    };
    if let Some(server) = &server {
        println!("the page is served from {}", server.url);
    }
    let url = server.as_ref().map(|s| s.url.as_str());
    in_app(&workspace, &plugin, &options, &app, &session, url)
}

/// The app's binary and how it is built and run, for a mode that runs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct App {
    /// The binary's name.
    pub(crate) bin: String,
    /// The features `cargo build` takes for it.
    pub(crate) features: String,
    /// The arguments the binary takes.
    pub(crate) args: Vec<String>,
}

impl App {
    /// Returns how `plugin`'s app is built and run for `options`, with its
    /// dev session in `session`.
    ///
    /// # Errors
    ///
    /// Says how to give the crate a standalone binary, when it has none.
    pub(crate) fn of(plugin: &Plugin, options: &Options, session: &Path) -> Result<App, String> {
        let bin = plugin.standalone.clone().ok_or_else(|| {
            format!(
                "{} has no standalone binary: a [[bin]] whose required-features are \
                 [\"standalone\"], calling hapto_standalone::main (docs/first-plugin.md, \
                 section 11)",
                plugin.package
            )
        })?;
        let mut features = String::from("standalone");
        let mut args = vec!["--dev".to_owned(), session.display().to_string()];
        if options.mode == Mode::DevHost {
            features.push_str(",hapto-standalone/dev-host");
            args.push("--host".to_owned());
        }
        if let Some(signal) = &options.signal {
            args.extend(["--signal".to_owned(), signal.clone()]);
        }
        Ok(App {
            bin,
            features,
            args,
        })
    }
}

fn in_app(
    workspace: &Workspace,
    plugin: &Plugin,
    options: &Options,
    app: &App,
    session: &Path,
    url: Option<&str>,
) -> Result<(), String> {
    let watch = Watch::new(&workspace.root, workspace.member_dirs());
    // Taken before the first build, which can take minutes, so a change
    // saved while it runs starts the next.
    let mut built = watch.fingerprint();
    let (mut exe, _) = build_app(workspace, plugin, app, options.profile)?;
    let mut copies = Copies::new(session.join("app"));
    let mut launch = |exe: &Path| Running::start(&copies.next(exe)?, &app.args, url);
    // The app that runs; none after a crash, until a build succeeds.
    let mut running = Some(launch(&exe)?);
    loop {
        std::thread::sleep(POLL);
        let ended = match running.as_mut() {
            Some(app) => app.ended()?,
            None => None,
        };
        match ended {
            Some(Ended::Closed) => return Ok(()),
            Some(Ended::Relaunch) => {
                running = Some(launch(&exe)?);
                continue;
            }
            Some(Ended::Failed(code)) => {
                eprintln!(
                    "the app ended with {}; the next build that succeeds starts it again",
                    exit_code(code)
                );
                running = None;
            }
            None => {}
        }
        let now = watch.fingerprint();
        if now == built {
            continue;
        }
        built = now;
        let build = match build_app(workspace, plugin, app, options.profile) {
            Ok((new, build)) => {
                exe = new;
                build
            }
            Err(error) => {
                eprintln!("{error}");
                Build::Failed
            }
        };
        // The app may have ended while it built, which can take minutes.
        let seen = match running.as_mut() {
            Some(app) => app.ended()?.map_or(Seen::Running, Seen::Ended),
            None => Seen::Stopped,
        };
        if let Seen::Ended(Ended::Failed(code)) = seen {
            eprintln!("the app ended with {} while it built", exit_code(code));
        }
        match after_build(seen, build) {
            Next::End => return Ok(()),
            Next::Start => {
                if let Some(mut app) = running.take() {
                    app.quit();
                }
                println!(
                    "{}",
                    match build {
                        Build::New => "rebuilt: starting the app with its saved block",
                        Build::Unchanged => "starting the app again with its saved block",
                        Build::Failed => "starting the app again with the last build, as it asked",
                    }
                );
                running = Some(launch(&exe)?);
            }
            Next::Keep if build == Build::Unchanged => {
                println!("the build is unchanged: the app plays on");
            }
            Next::Keep => eprintln!("the app runs the last build until the next change"),
            Next::Wait => {
                running = None;
                eprintln!("the next change builds again");
            }
        }
    }
}

/// Writes the page's map and builds the app's binary. Returns where Cargo
/// wrote it, as its messages say, and whether the build wrote it afresh.
///
/// # Errors
///
/// Returns the step that failed, or that the build wrote no such binary.
fn build_app(
    workspace: &Workspace,
    plugin: &Plugin,
    app: &App,
    profile: Profile,
) -> Result<(PathBuf, Build), String> {
    write_param_map(workspace, plugin)?;
    let artifacts = tool::build(
        cargo()
            .args(["build", "-p", &plugin.package, "--bin", &app.bin])
            .args(["--features", &app.features])
            .args(profile.cargo_args())
            .current_dir(&workspace.root),
    )?;
    artifacts
        .into_iter()
        .find(|a| a.target == app.bin && a.kinds.iter().any(|kind| kind == "bin"))
        .and_then(|a| {
            let build = if a.fresh {
                Build::Unchanged
            } else {
                Build::New
            };
            Some((a.executable?, build))
        })
        .ok_or_else(|| format!("cargo build wrote no {}", app.bin))
}

/// The command that runs the app at `exe` with `args`, its stdin piped,
/// reading its page from the dev server at `url`, or from none: a variable
/// the environment set is removed, so `--no-page` holds.
fn app_command(exe: &Path, args: &[String], url: Option<&str>) -> Command {
    let mut command = Command::new(exe);
    command.args(args).stdin(Stdio::piped());
    match url {
        Some(url) => command.env(PAGE_DEV_URL, url),
        None => command.env_remove(PAGE_DEV_URL),
    };
    command
}

/// Numbered copies of the built app, each run in place of the build itself:
/// on Windows a running executable cannot be replaced, so a rebuild while
/// the app runs would fail.
#[derive(Debug)]
pub(crate) struct Copies {
    dir: PathBuf,
    count: u32,
}

impl Copies {
    /// Copies builds into `dir`, one for each run, clearing the copies an
    /// earlier session left there, each a whole build, when Ctrl+C ended it
    /// before it could. A copy still running stays, and is passed over.
    pub(crate) fn new(dir: PathBuf) -> Copies {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let _ = std::fs::remove_file(entry.path());
        }
        Copies { dir, count: 0 }
    }

    /// Copies `built`, the build's binary, under the next number no file
    /// has, and returns the copy.
    ///
    /// # Errors
    ///
    /// Returns why the copy could not be made.
    pub(crate) fn next(&mut self, built: &Path) -> Result<PathBuf, String> {
        let stem = built.file_stem().unwrap_or_default().to_string_lossy();
        let copy = loop {
            self.count += 1;
            let copy = self.dir.join(format!(
                "{stem}-{}{}",
                self.count,
                std::env::consts::EXE_SUFFIX
            ));
            if !copy.exists() {
                break copy;
            }
        };
        std::fs::create_dir_all(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        std::fs::copy(built, &copy)
            .map_err(|e| format!("copying {} to {}: {e}", built.display(), copy.display()))?;
        Ok(copy)
    }
}

/// The app, running from its copy, with its stdin held: closing it asks the
/// app to quit. The copy is deleted once the app has ended.
#[derive(Debug)]
struct Running {
    child: Child,
    stdin: Option<ChildStdin>,
    exe: PathBuf,
}

impl Running {
    fn start(exe: &Path, args: &[String], url: Option<&str>) -> Result<Running, String> {
        let mut child = app_command(exe, args, url)
            .spawn()
            .map_err(|e| format!("{}: {e}", exe.display()))?;
        let stdin = child.stdin.take();
        Ok(Running {
            child,
            stdin,
            exe: exe.to_path_buf(),
        })
    }

    /// Returns how the app ended, once it has; `None` while it runs.
    fn ended(&mut self) -> Result<Option<Ended>, String> {
        let status = self.child.try_wait().map_err(|e| format!("the app: {e}"))?;
        Ok(status.map(|status| Ended::of(status.code())))
    }

    /// Asks the app to quit, keeping its saved block, and ends it if it has
    /// not within [`QUIT`].
    fn quit(&mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.flush();
        }
        let end = Instant::now() + QUIT;
        while Instant::now() < end {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.quit();
        let _ = std::fs::remove_file(&self.exe);
    }
}
