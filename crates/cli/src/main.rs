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
//! Cargo runs it as `cargo-hapto hapto <command> ...`, with the subcommand's
//! own name first; run any other way, as this repository's `cargo hapto`
//! alias does, the name is not there.

mod args;
mod bundle;
mod dev;
mod init;
mod page;
mod serve;
mod tool;
mod validate;
mod watch;
mod workspace;

use std::process::ExitCode;

fn usage() -> String {
    format!(
        "usage: cargo hapto <command>\n\n{}\n{}\n{}\n{}\n{}",
        init::USAGE,
        dev::USAGE,
        serve::USAGE,
        bundle::USAGE,
        validate::USAGE
    )
}

/// The usage of the command `name`, when it names one.
fn usage_of(name: &str) -> Option<&'static str> {
    match name {
        "init" => Some(init::USAGE),
        "dev" => Some(dev::USAGE),
        "serve" => Some(serve::USAGE),
        "build" => Some(bundle::USAGE),
        "validate" => Some(validate::USAGE),
        _ => None,
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().is_some_and(|arg| arg == "hapto") {
        args.next();
    }
    let command = args.next();
    let args: Vec<String> = args.collect();
    if let Some(usage) = command.as_deref().and_then(usage_of)
        && args::wants_help(&args)
    {
        println!("usage: cargo hapto\n\n{usage}");
        return ExitCode::SUCCESS;
    }
    let result = match command.as_deref() {
        Some("init") => init::run_command(args),
        Some("dev") => dev::run_command(args),
        Some("serve") => serve::run_command(args),
        Some("build") => bundle::run_command(args),
        Some("validate") => validate::run_command(args),
        Some("help" | "--help" | "-h") => {
            println!("{}", usage());
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("{}", usage());
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
