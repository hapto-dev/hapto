//! `cargo hapto validate`: pluginval on a plugin's release bundle, with the
//! options CI runs it with.

#[cfg(test)]
mod tests;

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;
use std::process::Command;

use crate::args::Args;
use crate::bundle::{self, Profile};

use crate::workspace::Workspace;

/// What `cargo hapto help` prints for this command.
pub(crate) const USAGE: &str =
    "  validate [-p <crate>] [--no-page] [--pluginval <path>] [--logs <folder>]
                    build the crate's release bundle and run pluginval on it as CI does: at
                    strictness level 5, in pluginval's own process, without its GUI tests;
                    pluginval is --pluginval, else PLUGINVAL, else the one on the PATH, and
                    --logs keeps its logs in a folder";

/// The options pluginval runs with, here and in CI, which runs this command:
/// the highest strictness the example EQ passes today, in pluginval's own
/// process, without its GUI tests.
pub(crate) const PLUGINVAL_ARGS: [&str; 4] = [
    "--strictness-level",
    "5",
    "--validate-in-process",
    "--skip-gui-tests",
];

/// What `cargo hapto validate` was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Options {
    /// The plugin crate, by its package name; `None` finds it.
    pub(crate) package: Option<String>,
    /// Whether to build the crate's page first.
    pub(crate) page: bool,
    /// The pluginval to run, when given.
    pub(crate) pluginval: Option<OsString>,
    /// The folder pluginval keeps its logs in, when given.
    pub(crate) logs: Option<OsString>,
}

impl Options {
    /// Reads the command's arguments, its own name excluded.
    ///
    /// # Errors
    ///
    /// Returns the argument it does not take.
    pub(crate) fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
        let mut args = Args::new(args, USAGE);
        let mut options = Options {
            package: None,
            page: true,
            pluginval: None,
            logs: None,
        };
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-p" | "--package" => options.package = Some(args.value(&arg)?),
                "--no-page" => options.page = false,
                "--pluginval" => options.pluginval = Some(args.value(&arg)?.into()),
                "--logs" => options.logs = Some(args.value(&arg)?.into()),
                _ => return Err(args.unknown(&arg)),
            }
        }
        Ok(options)
    }
}

/// Runs `cargo hapto validate` with the arguments after the command's name.
///
/// # Errors
///
/// Returns the step that failed: the build, or pluginval, which fails when
/// the bundle does not pass.
pub(crate) fn run_command(args: impl IntoIterator<Item = String>) -> Result<(), String> {
    let options = Options::parse(args)?;
    let (workspace, plugin) = Workspace::find(options.package.as_deref())?;
    let bundle = bundle::build(&workspace, &plugin, options.page, Profile::Release)?;
    let pluginval = options
        .pluginval
        .or_else(|| std::env::var_os("PLUGINVAL"))
        .unwrap_or_else(|| "pluginval".into());
    run_pluginval(&mut pluginval_command(
        &pluginval,
        &bundle,
        options.logs.as_deref(),
    ))
}

/// Runs `command`, a pluginval, and says where to find pluginval when it
/// cannot be found; one that runs and fails needs no such word.
///
/// # Errors
///
/// Returns that pluginval failed, or could not start, and why.
fn run_pluginval(command: &mut Command) -> Result<(), String> {
    println!("running pluginval");
    match command.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("pluginval failed: {status}")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(format!(
            "pluginval: {e}\npluginval is at https://github.com/Tracktion/pluginval: pass \
             --pluginval, set PLUGINVAL, or put it on the PATH"
        )),
        Err(e) => Err(format!("pluginval: {e}")),
    }
}

/// `pluginval` on `bundle` with [`PLUGINVAL_ARGS`], keeping its logs in
/// `logs` when given.
fn pluginval_command(pluginval: &OsStr, bundle: &Path, logs: Option<&OsStr>) -> Command {
    let mut command = Command::new(pluginval);
    command.args(PLUGINVAL_ARGS);
    if let Some(logs) = logs {
        command.arg("--output-dir").arg(logs);
    }
    command.arg(bundle);
    command
}
