//! The folder where a plugin keeps its data on the user's computer. The
//! standalone app keeps its session there, and the WebView its data.

#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

/// Returns the data folder of the plugin whose id is `id`, which is the folder
/// that [`plugin_folder`] names inside the user's data folder.
///
/// The user's data folder is:
///
/// - Windows: `%LOCALAPPDATA%`.
/// - macOS: `~/Library/Application Support`.
/// - Linux and every other system: `$XDG_DATA_HOME`, or `~/.local/share`
///   when that is unset or not absolute.
///
/// Every instance of the plugin gets the same folder. The function reads the
/// environment and makes no folder.
///
/// # Errors
///
/// Returns [`NoDataFolder`] when the variable that the user's data folder
/// comes from is unset or empty.
pub fn data_folder(id: &str) -> Result<PathBuf, NoDataFolder> {
    let base = user_data(std::env::consts::OS, std::env::var_os)?;
    Ok(plugin_folder(&base, id))
}

/// Returns the plugin's own folder below `base`, named after the plugin's id
/// `id`. Each character other than an ASCII letter, a digit, `.`, `-` and
/// `_` becomes `_`, so an id with `/` or `:` names one folder in `base`. An
/// empty id, or one of dots alone, becomes `hapto-plugin`, because `""`, `.`
/// and `..` name `base` itself or its parent.
#[must_use]
pub fn plugin_folder(base: &Path, id: &str) -> PathBuf {
    let name = id.replace(
        |c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')),
        "_",
    );
    if name.trim_matches('.').is_empty() {
        base.join("hapto-plugin")
    } else {
        base.join(name)
    }
}

/// Returns the user's runtime folder, `$XDG_RUNTIME_DIR`, which only the
/// user can open and which the system empties when the user logs out, or
/// `None` when that is unset, empty or not absolute. The WebView on Linux
/// keeps its data there when the user's data folder is unknown, and keeps its
/// short-lived folders there.
#[doc(hidden)]
#[must_use]
pub fn runtime_folder() -> Option<PathBuf> {
    runtime(std::env::var_os)
}

/// The error that [`data_folder`] returns when the environment variable that
/// names the user's data folder is unset or empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoDataFolder {
    /// The variable, such as `LOCALAPPDATA`.
    variable: &'static str,
}

impl fmt::Display for NoDataFolder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} is unset or empty", self.variable)
    }
}

impl std::error::Error for NoDataFolder {}

/// Returns the user's data folder on the operating system `os`, a value of
/// `std::env::consts::OS`, and reads variables through `var`.
fn user_data(
    os: &str,
    var: impl Fn(&'static str) -> Option<OsString>,
) -> Result<PathBuf, NoDataFolder> {
    // An empty variable names no folder.
    let path_in = |variable| {
        var(variable)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or(NoDataFolder { variable })
    };
    match os {
        "windows" => path_in("LOCALAPPDATA"),
        "macos" => Ok(path_in("HOME")?.join("Library").join("Application Support")),
        _ => xdg(&var, "XDG_DATA_HOME")
            .map_or_else(|| Ok(path_in("HOME")?.join(".local").join("share")), Ok),
    }
}

/// Returns the user's runtime folder, and reads variables through `var`.
fn runtime(var: impl Fn(&'static str) -> Option<OsString>) -> Option<PathBuf> {
    xdg(&var, "XDG_RUNTIME_DIR")
}

/// Returns the folder that the XDG variable `variable` names, read through
/// `var`, or `None` when it is unset or empty or names a relative path, which
/// XDG's specification says to ignore. On Unix, `has_root` and `is_absolute`
/// agree, but on Windows only `has_root` accepts `/data/a`, so these rules'
/// tests run there too.
fn xdg(var: &impl Fn(&'static str) -> Option<OsString>, variable: &'static str) -> Option<PathBuf> {
    var(variable)
        .map(PathBuf::from)
        .filter(|path| path.has_root())
}
