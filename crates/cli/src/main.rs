//! `cargo hapto <command>`: the plugin author's command for a Hapto plugin
//! crate, from any workspace.
//!
//! - `init` writes a new plugin crate, with fresh ids.
//! - `dev` runs a plugin while its author edits it: standalone or in the
//!   dev host, with the page from its dev server.
//! - `serve` runs the page's dev server alone, for a debug bundle in a real
//!   host.
//! - `build` builds a plugin's page and module and lays out its VST3 bundle,
//!   and installs it when asked.
//! - `validate` runs pluginval on that bundle as CI does.
//!
//! Each command's flags are declared once, for clap, which reads them, writes
//! the help and refuses what does not fit. Cargo runs it as `cargo-hapto
//! hapto <command> ...`, with the subcommand's own name first; run any other
//! way, as this repository's `cargo hapto` alias does, the name is not there.

#[cfg(test)]
mod tests;

mod bundle;
mod dev;
mod init;
mod page;
mod serve;
mod tool;
mod validate;
mod watch;
mod workspace;

use std::ffi::OsString;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// The plugin author's command for a Hapto plugin crate, from any workspace.
#[derive(Debug, Parser)]
#[command(name = "cargo hapto", bin_name = "cargo hapto", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// The commands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Write a new plugin crate in a folder: a gain with a bypass, its page, a
    /// standalone binary and tests, with fresh ids.
    ///
    /// The folder is new, empty, or holds `.git` alone; `.` is the current
    /// one. The crate takes its names from the folder's. Its Hapto crates
    /// come from the checkout `--hapto` names, by default the one this
    /// command was built from.
    Init(init::Options),
    /// Run the plugin standalone, rebuilding and restarting it with its saved
    /// block kept after each change to Rust, with the page from its dev
    /// server.
    ///
    /// `--host` runs it through the dev host instead: the VST3 exporter or
    /// the C interface, with a settings window of what the host sees.
    Dev(dev::Line),
    /// Serve the crate's page from its dev server until Ctrl+C, for its debug
    /// bundle in a real host.
    ///
    /// Install the bundle with `cargo hapto build --debug --install`, and
    /// start the host with `HAPTO_PAGE_DEV_URL` set to the address this
    /// prints: the page reloads as you edit it.
    Serve(serve::Options),
    /// Check and build the crate's page, build the crate in release, and lay
    /// out its VST3 bundle under `target/bundle/`.
    Build(bundle::Line),
    /// Build the crate's release bundle and run pluginval on it as CI does: at
    /// strictness level 5, in pluginval's own process, without its GUI tests.
    ///
    /// Pluginval is `--pluginval`, else `PLUGINVAL`, else the one on the
    /// PATH.
    Validate(validate::Options),
}

/// The command line without the subcommand's name Cargo puts first.
fn command_line() -> Vec<OsString> {
    let mut args: Vec<OsString> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == "hapto") {
        args.remove(1);
    }
    args
}

fn main() -> ExitCode {
    let cli = Cli::parse_from(command_line());
    let result = match cli.command {
        Command::Init(options) => init::run(&options),
        Command::Dev(line) => dev::run(&line.into()),
        Command::Serve(options) => serve::run(&options),
        Command::Build(line) => bundle::run(&line.into()),
        Command::Validate(options) => validate::run(options),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // The alternate form prints the whole chain: the step, then
            // each cause.
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Reads `args` as one command's flags, its name left out: for the
/// commands' own tests.
#[cfg(test)]
fn parse_as<T: clap::Args>(args: &[&str]) -> Result<T, clap::Error> {
    /// One command's flags alone.
    #[derive(Debug, Parser)]
    #[command(no_binary_name = true)]
    struct One<T: clap::Args> {
        #[command(flatten)]
        flags: T,
    }
    One::<T>::try_parse_from(args).map(|one| one.flags)
}
