//! `cargo hapto build`: a plugin crate's VST3 bundle, built and laid out in
//! one command, in any shell.
//!
//! It builds the crate's page when the crate has one, then the crate in
//! release, or in debug with `--debug`, then lays the bundle out under
//! `target/bundle/` and prints its path; with `--install` or
//! `--install-system` it copies the bundle into a VST3 folder the host scans.
//! Windows only for now: a macOS bundle has another layout.

#[cfg(test)]
mod tests;

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::args::Args;
use crate::page::{self, PAGE_DEV_URL};
use crate::tool::{self, Artifact, cargo};
use crate::workspace::{Plugin, Workspace};

/// What `cargo hapto help` prints for this command.
pub(crate) const USAGE: &str =
    "  build [-p <crate>] [--no-page] [--debug] [--install | --install-system]
                    check and build the crate's page, build the crate in release, lay out its
                    VST3 bundle under target/bundle/ and print its path; --debug bundles the
                    debug build, --install copies the bundle to the per-user VST3 folder,
                    --install-system to the one in Program Files";

/// The Windows error a copy over a file another process holds fails with:
/// `ERROR_SHARING_VIOLATION`.
const SHARING_VIOLATION: i32 = 32;

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

impl Options {
    /// Reads the command's arguments, its own name excluded.
    ///
    /// # Errors
    ///
    /// Returns what is wrong with them: an unknown flag, or both install
    /// flags.
    pub(crate) fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
        let mut args = Args::new(args, USAGE);
        let mut options = Options {
            package: None,
            page: true,
            profile: Profile::Release,
            install: None,
        };
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-p" | "--package" => options.package = Some(args.value(&arg)?),
                "--no-page" => options.page = false,
                "--debug" => options.profile = Profile::Debug,
                "--install" | "--install-system" => {
                    let folder = if arg == "--install" {
                        Folder::User
                    } else {
                        Folder::System
                    };
                    if options.install.replace(folder).is_some_and(|f| f != folder) {
                        return Err("--install and --install-system: pick one".into());
                    }
                }
                _ => return Err(args.unknown(&arg)),
            }
        }
        Ok(options)
    }
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

/// A VST3 folder a Windows host scans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Folder {
    /// `%LOCALAPPDATA%\Programs\Common\VST3`, which needs no administrator.
    User,
    /// `%CommonProgramFiles%\VST3`, which needs an administrator's shell.
    System,
}

impl Folder {
    /// Returns the folder's path, from the environment.
    ///
    /// # Errors
    ///
    /// Returns which variable is not set.
    pub(crate) fn path(self) -> Result<PathBuf, String> {
        let (variable, under) = match self {
            Folder::User => ("LOCALAPPDATA", ["Programs", "Common", "VST3"].as_slice()),
            Folder::System => ("CommonProgramFiles", ["VST3"].as_slice()),
        };
        let base = std::env::var_os(variable).ok_or(format!("{variable} is not set"))?;
        Ok(under
            .iter()
            .fold(PathBuf::from(base), |path, part| path.join(part)))
    }

    /// Returns the other folder.
    pub(crate) fn other(self) -> Folder {
        match self {
            Folder::User => Folder::System,
            Folder::System => Folder::User,
        }
    }
}

/// Returns the folder under `Contents` that holds the module, for the
/// machine the header of `module`, a Windows DLL's first bytes, names: the
/// one a host of that architecture looks in, whatever this command was built
/// for.
///
/// # Errors
///
/// Says the bytes are no DLL, or name a machine no folder is known for.
pub(crate) fn arch_folder(module: &[u8]) -> Result<&'static str, String> {
    let at = module
        .get(0x3c..0x40)
        .filter(|_| module.starts_with(b"MZ"))
        .and_then(|bytes| usize::try_from(u32::from_le_bytes(bytes.try_into().ok()?)).ok());
    let machine = at
        .and_then(|at| {
            let pe = module.get(at..at.checked_add(4)?)?;
            let machine = module.get(at.checked_add(4)?..at.checked_add(6)?)?;
            (pe == b"PE\0\0").then(|| u16::from_le_bytes([machine[0], machine[1]]))
        })
        .ok_or("the built module is no Windows DLL")?;
    match machine {
        0x8664 => Ok("x86_64-win"),
        0xaa64 => Ok("arm64-win"),
        other => Err(format!(
            "no VST3 bundle folder is known for the machine {other:#06x} the module is \
             built for"
        )),
    }
}

/// Refuses a system with no bundle layout yet: Windows alone has one.
///
/// # Errors
///
/// Says the bundle's layout on `os` is not built yet.
pub(crate) fn windows_only(os: &str) -> Result<(), String> {
    if os == "windows" {
        Ok(())
    } else {
        Err(format!(
            "build lays out a Windows bundle only; on {os} a VST3 bundle has another \
             layout, which is not built yet"
        ))
    }
}

/// Returns the first bytes of the file at `path`, enough to hold a DLL's
/// headers.
///
/// # Errors
///
/// Returns why it could not be read.
pub(crate) fn head(path: &Path) -> Result<Vec<u8>, String> {
    let mut head = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(64 * 1024).read_to_end(&mut head))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(head)
}

/// Returns the bundle's folder under `root` and the path of the module in
/// it, both named by the plugin's [`file_name`].
///
/// # Errors
///
/// Returns why `name` names no file.
pub(crate) fn layout(root: &Path, name: &str, arch: &str) -> Result<(PathBuf, PathBuf), String> {
    let file = format!("{}.vst3", file_name(name)?);
    let bundle = root.join(&file);
    let module = bundle.join("Contents").join(arch).join(&file);
    Ok((bundle, module))
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
pub(crate) fn file_name(name: &str) -> Result<String, String> {
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
        return Err(format!(
            "the plugin's name {name:?} leaves no file name for its bundle"
        ));
    }
    // Windows takes a device's name with anything after a dot for the
    // device, and spaces before the dot with it.
    let base = mapped.split('.').next().unwrap_or_default().trim_end();
    if DEVICES
        .iter()
        .any(|device| base.eq_ignore_ascii_case(device))
    {
        return Err(format!(
            "the plugin's name {name:?} is a device name Windows keeps, so it cannot name \
             its bundle"
        ));
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

/// Runs `cargo hapto build` with the arguments after the command's name.
///
/// # Errors
///
/// Returns the step that failed and why.
pub(crate) fn run_command(args: impl IntoIterator<Item = String>) -> Result<(), String> {
    let options = Options::parse(args)?;
    let (workspace, plugin) = Workspace::find(options.package.as_deref())?;
    let bundle = build(&workspace, &plugin, options.page, options.profile)?;
    if let Some(folder) = options.install {
        install(&bundle, &plugin.name, folder)?;
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
/// bundle's folder.
///
/// # Errors
///
/// Returns the step that failed and why.
pub(crate) fn build(
    workspace: &Workspace,
    plugin: &Plugin,
    page: bool,
    profile: Profile,
) -> Result<PathBuf, String> {
    windows_only(std::env::consts::OS)?;
    // Before the builds, which take minutes, so a name no file can take is
    // refused at once.
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
    let arch = arch_folder(&head(&built)?)?;
    let (bundle, module) = layout(&workspace.target.join("bundle"), &plugin.name, arch)?;
    if file != plugin.name {
        println!(
            "the bundle is named {file:?}: a file name cannot hold all of {:?}",
            plugin.name
        );
    }
    lay_out(&built, &bundle, &module)?;
    println!("bundled {}", bundle.display());
    Ok(bundle)
}

/// Returns the module `plugin`'s build wrote, as its `artifacts` name it: the
/// `cdylib`'s DLL, wherever Cargo put it.
///
/// # Errors
///
/// Says the build named none.
pub(crate) fn module_of(artifacts: &[Artifact], plugin: &Plugin) -> Result<PathBuf, String> {
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
        .ok_or_else(|| format!("cargo build of {} wrote no {file}", plugin.package))
}

fn lay_out(built: &Path, bundle: &Path, module: &Path) -> Result<(), String> {
    if bundle.exists() {
        fs::remove_dir_all(bundle).map_err(|e| format!("{}: {e}", bundle.display()))?;
    }
    let contents = module.parent().ok_or("a module path has no folder")?;
    fs::create_dir_all(contents).map_err(|e| format!("{}: {e}", contents.display()))?;
    fs::copy(built, module).map_err(|e| format!("{}: {e}", built.display()))?;
    Ok(())
}

/// Copies the bundle into `folder`, and warns when the other folder holds a
/// bundle of the same name, since the host then picks one.
///
/// # Errors
///
/// Returns the copy's failure, worded by [`copy_error`].
pub(crate) fn install(bundle: &Path, name: &str, folder: Folder) -> Result<(), String> {
    let name = file_name(name)?;
    let into = folder.path()?.join(format!("{name}.vst3"));
    copy_tree(bundle, &into).map_err(|(path, e)| copy_error(&e, &path, folder))?;
    println!("installed {}", into.display());
    let other = folder.other().path()?.join(format!("{name}.vst3"));
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
