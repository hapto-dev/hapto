//! `cargo hapto validate`: pluginval on a plugin's release bundle, with the
//! options CI runs it with.

#[cfg(test)]
mod tests;

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;
use std::process::Command;

use clap::ArgAction;

use crate::bundle::{self, Profile};
use crate::workspace::Workspace;

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
#[derive(Clone, Debug, PartialEq, Eq, clap::Args)]
pub(crate) struct Options {
    /// The plugin crate, by its package name; else the one the folder is
    /// in, or the workspace's only one.
    #[arg(short, long, value_name = "CRATE")]
    pub(crate) package: Option<String>,
    /// Skip the page, for a Rust-only rebuild: the bundle has the page last
    /// built.
    #[arg(long = "no-page", action = ArgAction::SetFalse)]
    pub(crate) page: bool,
    /// The pluginval to run.
    #[arg(long, value_name = "PATH")]
    pub(crate) pluginval: Option<OsString>,
    /// A folder to keep pluginval's logs in.
    #[arg(long, value_name = "FOLDER")]
    pub(crate) logs: Option<OsString>,
}

/// Runs `cargo hapto validate` with `options`.
///
/// # Errors
///
/// Returns the step that failed: the build, or pluginval, which fails when
/// the bundle does not pass.
pub(crate) fn run(options: Options) -> Result<(), String> {
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
