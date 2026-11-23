//! `cargo hapto juce`: a plugin's settings for the JUCE adapter, which its
//! own `juce_settings` test writes from its `INFO`, and the CMake that builds
//! it through the adapter.

#[cfg(test)]
mod tests;

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};

use crate::init;
use crate::tool::{generating_test, run};
use crate::workspace::{Plugin, Workspace};

/// What `cargo hapto juce` was asked to do.
#[derive(Clone, Debug, PartialEq, Eq, clap::Args)]
pub(crate) struct Options {
    /// The plugin crate, by its package name; else the one the folder is
    /// in, or the workspace's only one.
    #[arg(short, long, value_name = "CRATE")]
    pub(crate) package: Option<String>,
}

/// Runs `cargo hapto juce` with `options`: writes the settings, or in CI
/// checks them, and prints the CMake that builds the plugin through the
/// adapter.
///
/// # Errors
///
/// Returns that the crate has no `juce_settings` test, or the test's
/// failure: a parameter key whose JUCE id collides, or in CI settings that
/// do not match the plugin's `INFO`.
pub(crate) fn run_juce(options: &Options) -> Result<()> {
    let (workspace, plugin) = Workspace::find(options.package.as_deref())?;
    if !plugin.has_juce_settings_test {
        bail!(
            "{} has no `juce_settings` test, which writes its settings from its INFO: add \
             tests/juce_settings.rs, as `cargo hapto init` writes it",
            plugin.package
        );
    }
    let ci = std::env::var_os("CI");
    run(
        &mut settings_test(&plugin.package, &workspace.root, ci.as_deref()),
        "the JUCE adapter's settings",
    )?;
    let adapter = format!("{}/adapters/juce/HaptoJuce.cmake", init::checkout(None)?);
    println!("{}", cmake(&plugin, &workspace.root, &adapter));
    Ok(())
}

/// The `juce_settings` test of `package`, run from `root`, with `ci` the
/// value of `CI`, as [`generating_test`] runs it: it writes the settings, or
/// in CI checks them.
fn settings_test(package: &str, root: &Path, ci: Option<&OsStr>) -> Command {
    generating_test(package, "juce_settings", root, ci)
}

/// What a CMake project that adds JUCE writes to build `plugin`, of the
/// workspace at `root`, through the adapter at `adapter`.
fn cmake(plugin: &Plugin, root: &Path, adapter: &str) -> String {
    let slashes = |path: &Path| path.to_string_lossy().replace('\\', "/");
    let target: String = plugin
        .name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    format!(
        "{settings} holds the adapter's settings. A CMake project that adds JUCE first builds the \
         plugin through the adapter with:\n\n\
         include(\"{adapter}\")\n\
         hapto_add_juce_plugin({target}\n    \
         PACKAGE {package}\n    \
         SETTINGS \"{settings}\"\n    \
         MANIFEST_PATH \"{manifest}\"\n    \
         FORMATS VST3 AU)\n\n\
         The adapter must be the one beside the hapto-host the plugin builds against.",
        settings = slashes(&plugin.dir.join("hapto-juce.cmake")),
        package = plugin.package,
        manifest = slashes(&root.join("Cargo.toml")),
    )
}
