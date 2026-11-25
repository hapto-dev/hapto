//! The layout of a VST3 bundle and the folders that it installs to, for each
//! module format that has one.

#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use super::format::{Arch, Format};

/// A VST3 folder that hosts scan, whose path [`Layout::install_folder`]
/// returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Folder {
    /// The user's folder, which needs no administrator.
    User,
    /// The folder for everyone, which needs an administrator's shell.
    System,
}

impl Folder {
    /// Returns the other folder.
    pub(crate) fn other(self) -> Folder {
        match self {
            Folder::User => Folder::System,
            Folder::System => Folder::User,
        }
    }
}

/// How a VST3 bundle holds a module of one format, and where the bundle is
/// installed.
#[derive(Debug)]
pub(super) struct Layout {
    /// The folder under `Contents` for a module built for a given processor,
    /// such as `x86_64-win`.
    folder: fn(Arch) -> &'static str,
    /// The suffix that follows the plugin's file name in the module's name,
    /// such as `.vst3`.
    suffix: &'static str,
    /// The user's VST3 folder.
    user: Place,
    /// The VST3 folder for everyone.
    system: Place,
}

/// A folder under the value of an environment variable.
#[derive(Debug)]
struct Place {
    /// The environment variable's name.
    variable: &'static str,
    /// The folders under it, outermost first.
    under: &'static [&'static str],
}

/// The layout for a PE module, on Windows.
static PE: Layout = Layout {
    folder: |arch| match arch {
        Arch::X86_64 => "x86_64-win",
        Arch::Aarch64 => "arm64-win",
    },
    // A Windows module is the DLL, renamed with the bundle's extension.
    suffix: ".vst3",
    user: Place {
        variable: "LOCALAPPDATA",
        under: &["Programs", "Common", "VST3"],
    },
    system: Place {
        variable: "CommonProgramFiles",
        under: &["VST3"],
    },
};

impl Layout {
    /// Returns the layout for modules of `format`.
    ///
    /// # Errors
    ///
    /// Returns an error that names `format` if it has no layout yet.
    pub(super) fn of(format: Format) -> Result<&'static Layout> {
        match format {
            Format::Pe => Ok(&PE),
            Format::Elf | Format::MachO => {
                bail!("cargo hapto has no VST3 bundle layout for {format} modules yet")
            }
        }
    }

    /// Returns the bundle's folder under `root` and the path of its module for
    /// `arch`, both named by `file`, the plugin's
    /// [`file_name`](super::file_name).
    pub(super) fn paths(&self, root: &Path, file: &str, arch: Arch) -> (PathBuf, PathBuf) {
        let bundle = root.join(format!("{file}.vst3"));
        let module = bundle
            .join("Contents")
            .join((self.folder)(arch))
            .join(format!("{file}{}", self.suffix));
        (bundle, module)
    }

    /// Returns the path of `folder`, with `var` reading the environment.
    ///
    /// # Errors
    ///
    /// Returns which variable is not set.
    pub(super) fn install_folder(
        &self,
        folder: Folder,
        var: impl Fn(&'static str) -> Option<OsString>,
    ) -> Result<PathBuf> {
        let place = match folder {
            Folder::User => &self.user,
            Folder::System => &self.system,
        };
        let base = var(place.variable).with_context(|| format!("{} is not set", place.variable))?;
        Ok(place
            .under
            .iter()
            .fold(PathBuf::from(base), |path, part| path.join(part)))
    }
}
