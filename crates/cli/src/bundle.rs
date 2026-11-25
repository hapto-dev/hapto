//! `cargo hapto build`: a plugin crate's VST3 bundle, built and laid out in
//! one command, in any shell.
//!
//! It builds the crate's page when the crate has one, then the crate in
//! release, or in debug with `--debug`, then lays the bundle out under
//! `target/bundle/` and prints its path; with `--install` or
//! `--install-system` it copies the bundle into a VST3 folder the host scans.
//! [`Layout::of`] returns the bundle's layout for the module's file format.

mod format;
mod layout;
#[cfg(test)]
mod tests;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};

use self::format::Format;
use self::layout::{Folder, Layout};
use crate::page::{self, PAGE_DEV_URL};
use crate::tool::{self, Artifact, cargo};
use crate::workspace::{Plugin, Workspace};

/// The Windows error a copy over a file another process holds fails with:
/// `ERROR_SHARING_VIOLATION`.
const SHARING_VIOLATION: i32 = 32;

/// What `cargo hapto build` reads from its command line.
#[derive(Clone, Debug, PartialEq, Eq, clap::Args)]
pub(crate) struct Line {
    /// The plugin crate, by its package name; else the one the folder is
    /// in, or the workspace's only one.
    #[arg(short, long, value_name = "CRATE")]
    package: Option<String>,
    /// Skip the page, for a Rust-only rebuild: the bundle has the page last
    /// built.
    #[arg(long)]
    no_page: bool,
    /// Bundle the debug build, which loads its page from a dev server.
    #[arg(long)]
    debug: bool,
    /// Copy the bundle to the per-user VST3 folder.
    #[arg(long, conflicts_with = "install_system")]
    install: bool,
    /// Copy the bundle to the VST3 folder in Program Files, from an
    /// administrator's shell.
    #[arg(long)]
    install_system: bool,
}

impl From<Line> for Options {
    fn from(line: Line) -> Options {
        Options {
            package: line.package,
            page: !line.no_page,
            profile: if line.debug {
                Profile::Debug
            } else {
                Profile::Release
            },
            install: match (line.install, line.install_system) {
                (true, _) => Some(Folder::User),
                (false, true) => Some(Folder::System),
                (false, false) => None,
            },
        }
    }
}

/// What `cargo hapto build` was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Options {
    /// The plugin crate, by its package name; `None` finds it.
    pub(crate) package: Option<String>,
    /// Whether to build the crate's page first; `--no-page` turns it off.
    pub(crate) page: bool,
    /// The build the module comes from; `--debug` picks the debug one.
    pub(crate) profile: Profile,
    /// The VST3 folder to copy the bundle into, if any.
    pub(crate) install: Option<Folder>,
}

/// The Cargo build a bundle's module comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Profile {
    /// The optimised build, for listening and for shipping.
    Release,
    /// The debug build, which loads its page from a dev server when the
    /// host's environment sets `HAPTO_PAGE_DEV_URL`, so the page reloads live
    /// in the host. Its DSP runs unoptimised.
    Debug,
}

impl Profile {
    /// Returns the arguments `cargo build` takes for it.
    pub(crate) fn cargo_args(self) -> &'static [&'static str] {
        match self {
            Profile::Release => &["--release"],
            Profile::Debug => &[],
        }
    }
}

/// A bundle that [`build`] laid out.
#[derive(Debug)]
pub(crate) struct Bundle {
    /// The bundle's folder.
    pub(crate) path: PathBuf,
    /// The bundle's layout, which holds the folders that it installs to.
    layout: &'static Layout,
}

/// The characters a Windows file name cannot hold, besides control
/// characters.
const NOT_IN_A_FILE_NAME: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// The device names Windows keeps, whatever follows them after a dot, the
/// superscript digits among them.
const DEVICES: &[&str] = &[
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "COM0",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9",
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT0",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
];

/// Returns the name a plugin's bundle and module take on disk: its name,
/// which hosts show, with each character a Windows file name cannot hold
/// written as `-`, and trailing dots and spaces dropped. So a name with a
/// `/` or a `:` still names one file in the bundle's folder, and never a
/// path out of it.
///
/// # Errors
///
/// Returns why the name names no file: nothing is left of it, or it is a
/// device name Windows keeps.
pub(crate) fn file_name(name: &str) -> Result<String> {
    let mapped: String = name
        .chars()
        .map(|c| {
            if c.is_control() || NOT_IN_A_FILE_NAME.contains(&c) {
                '-'
            } else {
                c
            }
        })
        .collect();
    let mapped = mapped.trim_end_matches(['.', ' ']).to_owned();
    if mapped.trim().is_empty() {
        bail!("the plugin's name {name:?} leaves no file name for its bundle");
    }
    // Windows takes a device's name with anything after a dot for the
    // device, and spaces before the dot with it.
    let base = mapped.split('.').next().unwrap_or_default().trim_end();
    if DEVICES
        .iter()
        .any(|device| base.eq_ignore_ascii_case(device))
    {
        bail!(
            "the plugin's name {name:?} is a device name Windows keeps, so it cannot name \
             its bundle"
        );
    }
    Ok(mapped)
}

/// Words a failed copy into `folder` with: the host holding the module, the
/// folder needing an administrator, or the error itself.
pub(crate) fn copy_error(error: &io::Error, path: &Path, folder: Folder) -> String {
    let path = path.display();
    if error.raw_os_error() == Some(SHARING_VIOLATION) {
        format!(
            "{path}: {error}\nquit the host first: it keeps the module in use once an \
             editor has opened"
        )
    } else if error.kind() == io::ErrorKind::PermissionDenied && folder == Folder::System {
        format!(
            "{path}: {error}\n--install-system needs an administrator's shell; --install \
             copies to the per-user folder, which needs none"
        )
    } else {
        format!("{path}: {error}")
    }
}

/// Runs `cargo hapto build` with `options`.
///
/// # Errors
///
/// Returns the step that failed and why.
pub(crate) fn run(options: &Options) -> Result<()> {
    let (workspace, plugin) = Workspace::find(options.package.as_deref())?;
    let bundle = build(&workspace, &plugin, options.page, options.profile)?;
    if let Some(folder) = options.install {
        install(&bundle, folder)?;
    }
    if options.profile == Profile::Debug {
        println!(
            "a debug build: run cargo hapto serve, and start the host with \
             {PAGE_DEV_URL} set to the address it prints, and the page reloads live"
        );
    }
    Ok(())
}

/// Builds `plugin`'s page, when `page` holds and it has one, and its module
/// in `profile`, and lays out its bundle under `target/bundle/`. Returns the
/// bundle.
///
/// # Errors
///
/// Returns the step that failed and why.
pub(crate) fn build(
    workspace: &Workspace,
    plugin: &Plugin,
    page: bool,
    profile: Profile,
) -> Result<Bundle> {
    // These two checks come before the builds, which take minutes, so the
    // command fails at once on a system whose modules have no layout, or for a
    // name that no file can take.
    Layout::of(Format::of_system(std::env::consts::OS))?;
    let file = file_name(&plugin.name)?;
    match plugin.page() {
        None => println!("{} has no page", plugin.package),
        Some(dir) if page => page::prepare(workspace, plugin, &dir)?,
        Some(_) => println!("skipped the page (--no-page): the bundle has the page last built"),
    }
    let artifacts = tool::build(
        cargo()
            .args(["build", "-p", &plugin.package])
            .args(profile.cargo_args())
            .current_dir(&workspace.root),
    )?;
    let built = module_of(&artifacts, plugin)?;
    let (format, arch) = Format::read_file(&built)?;
    let layout = Layout::of(format)?;
    let (bundle, module) = layout.paths(&workspace.target.join("bundle"), &file, arch);
    if file != plugin.name {
        println!(
            "the bundle is named {file:?}: a file name cannot hold all of {:?}",
            plugin.name
        );
    }
    lay_out(&built, &bundle, &module)?;
    println!("bundled {}", bundle.display());
    Ok(Bundle {
        path: bundle,
        layout,
    })
}

/// Returns the module `plugin`'s build wrote, as its `artifacts` name it: the
/// `cdylib`'s library, wherever Cargo put it.
///
/// # Errors
///
/// Says the build named none.
pub(crate) fn module_of(artifacts: &[Artifact], plugin: &Plugin) -> Result<PathBuf> {
    let file = format!(
        "{}{}{}",
        std::env::consts::DLL_PREFIX,
        plugin.lib,
        std::env::consts::DLL_SUFFIX
    );
    artifacts
        .iter()
        .filter(|a| a.target == plugin.lib && a.kinds.iter().any(|kind| kind == "cdylib"))
        .flat_map(|a| &a.files)
        .find(|path| path.file_name().is_some_and(|name| *name == *file))
        .cloned()
        .with_context(|| format!("cargo build of {} wrote no {file}", plugin.package))
}

fn lay_out(built: &Path, bundle: &Path, module: &Path) -> Result<()> {
    if bundle.exists() {
        fs::remove_dir_all(bundle).with_context(|| bundle.display().to_string())?;
    }
    let contents = module.parent().context("a module path has no folder")?;
    fs::create_dir_all(contents).with_context(|| contents.display().to_string())?;
    fs::copy(built, module).with_context(|| built.display().to_string())?;
    Ok(())
}

/// Copies the bundle into `folder`, and warns when the other folder holds a
/// bundle of the same name, since the host then picks one.
///
/// # Errors
///
/// Returns the copy's failure, worded by [`copy_error`].
pub(crate) fn install(bundle: &Bundle, folder: Folder) -> Result<()> {
    let name = bundle
        .path
        .file_name()
        .context("a bundle's path has no folder name")?;
    let into = bundle
        .layout
        .install_folder(folder, std::env::var_os)?
        .join(name);
    copy_tree(&bundle.path, &into).map_err(|(path, e)| anyhow!(copy_error(&e, &path, folder)))?;
    println!("installed {}", into.display());
    let other = bundle
        .layout
        .install_folder(folder.other(), std::env::var_os)?
        .join(name);
    if other.exists() {
        eprintln!(
            "warning: {} is there too; with two copies the host picks one",
            other.display()
        );
    }
    Ok(())
}

/// Copies the folder `from` over `to`, file by file, and returns the path
/// that failed with its error.
fn copy_tree(from: &Path, to: &Path) -> Result<(), (PathBuf, io::Error)> {
    fs::create_dir_all(to).map_err(|e| (to.to_path_buf(), e))?;
    for entry in fs::read_dir(from).map_err(|e| (from.to_path_buf(), e))? {
        let entry = entry.map_err(|e| (from.to_path_buf(), e))?;
        let (source, dest) = (entry.path(), to.join(entry.file_name()));
        if source.is_dir() {
            copy_tree(&source, &dest)?;
        } else {
            fs::copy(&source, &dest).map_err(|e| (dest.clone(), e))?;
        }
    }
    Ok(())
}
