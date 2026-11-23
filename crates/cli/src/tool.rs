//! The programs a command runs: Cargo, npm and Node.

#[cfg(test)]
mod tests;

use std::ffi::OsStr;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail, ensure};
use serde_json::Value as Json;

/// What `cargo build` made of one target, as its JSON messages say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Artifact {
    /// The target's name: a library's crate name, or a binary's.
    pub(crate) target: String,
    /// The target's kinds: `bin`, `cdylib`, `rlib` and so on.
    pub(crate) kinds: Vec<String>,
    /// The files it holds, where Cargo wrote them.
    pub(crate) files: Vec<PathBuf>,
    /// A binary's executable.
    pub(crate) executable: Option<PathBuf>,
    /// Whether Cargo found it up to date and wrote nothing.
    pub(crate) fresh: bool,
}

impl Artifact {
    /// Reads one line of Cargo's JSON messages: the artifact it names, if
    /// it names one.
    pub(crate) fn read(line: &str) -> Option<Artifact> {
        let message: Json = serde_json::from_str(line).ok()?;
        if message.get("reason")?.as_str()? != "compiler-artifact" {
            return None;
        }
        let strings = |value: &Json| -> Vec<String> {
            value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        };
        Some(Artifact {
            target: message["target"]["name"].as_str()?.to_owned(),
            kinds: strings(&message["target"]["kind"]),
            files: strings(&message["filenames"])
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            executable: message["executable"].as_str().map(PathBuf::from),
            fresh: message["fresh"].as_bool().unwrap_or(false),
        })
    }
}

/// Runs `command`, a `cargo build`, and returns the artifacts it made, as
/// its messages name them, so a command finds each file where Cargo wrote
/// it, whatever target folder and target it was given. Its diagnostics are
/// shown as Cargo renders them.
///
/// # Errors
///
/// Returns that the build failed, and how, when it could not start or did
/// not succeed.
pub(crate) fn build(command: &mut Command) -> Result<Vec<Artifact>> {
    println!("running cargo build");
    let mut child = command
        .arg("--message-format=json-render-diagnostics")
        .stdout(Stdio::piped())
        .spawn()
        .context("cargo build")?;
    let mut artifacts = Vec::new();
    let mut unread = None;
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => artifacts.extend(Artifact::read(&line)),
                Err(e) => {
                    unread = Some(anyhow::Error::new(e).context("cargo build's messages"));
                    let _ = child.kill();
                    break;
                }
            }
        }
    }
    let status = child.wait().context("cargo build")?;
    match unread {
        Some(error) => Err(error),
        None if status.success() => Ok(artifacts),
        None => bail!("cargo build failed: {status}"),
    }
}

/// Returns the Cargo that started this command, so a toolchain override
/// holds: Cargo names itself in `CARGO` for its subcommands.
pub(crate) fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

/// Returns npm. It is a script, `npm.cmd`, on Windows, and a bare name finds
/// only an `.exe` there.
pub(crate) fn npm() -> Command {
    Command::new(if cfg!(windows) { "npm.cmd" } else { "npm" })
}

/// The test `test` of `package`, run from `root`, with `ci` the value of
/// `CI`: a plugin's generated file is its own test's to write, since a tool
/// that linked the plugin would carry its entry points. The test writes the
/// file, unless `CI` is set: there it checks it, since Hapto's drift check
/// refuses a test that rewrites its own file in CI, and a file that does not
/// match fails the build. The variables' names are `hapto-testkit`'s, which
/// this command does not link. Cargo shows its progress, since a new crate's
/// first run compiles hundreds of crates here, and the test's own harness is
/// quiet.
pub(crate) fn generating_test(
    package: &str,
    test: &str,
    root: &Path,
    ci: Option<&OsStr>,
) -> Command {
    let mut command = cargo();
    command
        .args(["test", "-p", package, "--test", test, "--", "-q"])
        .current_dir(root);
    if ci.is_none_or(OsStr::is_empty) {
        command.env("HAPTO_BLESS", "1");
    }
    command
}

/// Runs `command`, its output shown, and says `what` it runs first.
///
/// # Errors
///
/// Returns `what` failed, and how, when it could not start or did not
/// succeed.
pub(crate) fn run(command: &mut Command, what: &str) -> Result<()> {
    println!("running {what}");
    let status = command.status().with_context(|| what.to_owned())?;
    ensure!(status.success(), "{what} failed: {status}");
    Ok(())
}

/// Runs `command` and returns what it wrote to stdout.
///
/// # Errors
///
/// Returns `what` failed and its stderr, when it could not start or did not
/// succeed.
pub(crate) fn output(command: &mut Command, what: &str) -> Result<String> {
    let output = command.output().with_context(|| what.to_owned())?;
    ensure!(
        output.status.success(),
        "{what}: {}",
        String::from_utf8_lossy(&output.stderr).trim_end()
    );
    String::from_utf8(output.stdout).with_context(|| what.to_owned())
}
