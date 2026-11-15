//! `cargo xtask bundle`: a plugin crate's VST3 bundle, built and laid out in
//! one command, in any shell.
//!
//! It builds the crate's page when the crate has one, then the crate in
//! release, then lays the bundle out under `target/bundle/` and prints its
//! path; with `--install` or `--install-system` it copies the bundle into a
//! VST3 folder the host scans. A crate's page is the npm package in its
//! `page` folder. The page is type-checked and built every time, so a page
//! that does not check, or a missing or stale build, never reaches a bundle
//! unseen; Cargo itself never runs Node, so this task does. Windows only for
//! now: a macOS bundle has another layout.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// The usage line `cargo xtask` prints for this task.
pub const USAGE: &str = "  bundle -p <crate> [--no-page] [--install | --install-system]
                    check and build the crate's page, build the crate in release, lay out its
                    VST3 bundle under target/bundle/ and print its path; --install copies it to
                    the per-user VST3 folder, --install-system to the one in Program Files";

/// The Windows error a copy over a file another process holds fails with:
/// `ERROR_SHARING_VIOLATION`.
const SHARING_VIOLATION: i32 = 32;

/// What `cargo xtask bundle` was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// The plugin crate, by its package name.
    pub package: String,
    /// Whether to build the crate's page first; `--no-page` turns it off.
    pub page: bool,
    /// The VST3 folder to copy the bundle into, if any.
    pub install: Option<Folder>,
}

impl Options {
    /// Reads the task's arguments, the task's own name excluded.
    ///
    /// # Errors
    ///
    /// Returns what is wrong with them: an unknown flag, no crate, or both
    /// install flags.
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
        let mut package = None;
        let mut page = true;
        let mut install = None;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-p" | "--package" => {
                    package = Some(args.next().ok_or("-p wants a crate's name")?);
                }
                "--no-page" => page = false,
                "--install" | "--install-system" => {
                    let folder = if arg == "--install" {
                        Folder::User
                    } else {
                        Folder::System
                    };
                    if install.replace(folder).is_some_and(|f| f != folder) {
                        return Err("--install and --install-system: pick one".into());
                    }
                }
                _ => return Err(format!("bundle does not take {arg:?}\n\n{USAGE}")),
            }
        }
        let package = package.ok_or_else(|| format!("bundle wants -p <crate>\n\n{USAGE}"))?;
        Ok(Options {
            package,
            page,
            install,
        })
    }
}

/// A VST3 folder a Windows host scans.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Folder {
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
    pub fn path(self) -> Result<PathBuf, String> {
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
    pub fn other(self) -> Folder {
        match self {
            Folder::User => Folder::System,
            Folder::System => Folder::User,
        }
    }
}

/// A plugin crate, as `cargo metadata` describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plugin {
    /// The name every host shows, from `[package.metadata.seam]`; the
    /// bundle's name too.
    pub name: String,
    /// The library's name, which names the file the build writes.
    pub lib: String,
    /// The crate's folder.
    pub dir: PathBuf,
}

/// Finds `package` in the output of `cargo metadata --format-version 1`, and
/// returns it with the workspace's target folder.
///
/// # Errors
///
/// Returns why it is no plugin to bundle: not in the workspace, no `cdylib`,
/// or no name in `[package.metadata.seam]`.
pub fn plugin(metadata: &serde_json::Value, package: &str) -> Result<(Plugin, PathBuf), String> {
    let target = metadata["target_directory"]
        .as_str()
        .ok_or("cargo metadata gave no target_directory")?;
    let found = metadata["packages"]
        .as_array()
        .ok_or("cargo metadata gave no packages")?
        .iter()
        .find(|p| p["name"] == package)
        .ok_or_else(|| format!("{package} is not a crate of this workspace"))?;
    let lib = found["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| {
            t["kind"]
                .as_array()
                .is_some_and(|k| k.iter().any(|k| k == "cdylib"))
        })
        .and_then(|t| t["name"].as_str())
        .ok_or_else(|| format!("{package} builds no cdylib, so it has no module to bundle"))?;
    let name = found["metadata"]["seam"]["name"]
        .as_str()
        .ok_or_else(|| format!("{package} has no name in [package.metadata.seam]"))?;
    let manifest = found["manifest_path"]
        .as_str()
        .ok_or("cargo metadata gave a package no manifest_path")?;
    let dir = Path::new(manifest)
        .parent()
        .ok_or("a manifest path has no folder")?;
    let plugin = Plugin {
        name: name.to_owned(),
        lib: lib.replace('-', "_"),
        dir: dir.to_path_buf(),
    };
    Ok((plugin, PathBuf::from(target)))
}

/// Returns the folder under `Contents` that holds the module, for the
/// operating system and architecture `std::env::consts` names.
///
/// # Errors
///
/// Returns why there is no bundle for them yet.
pub fn arch_folder(os: &str, arch: &str) -> Result<&'static str, String> {
    match (os, arch) {
        ("windows", "x86_64") => Ok("x86_64-win"),
        ("windows", "aarch64") => Ok("arm64-win"),
        ("windows", _) => Err(format!("no VST3 bundle folder is known for {arch}")),
        _ => Err(format!(
            "bundle lays out a Windows bundle only; on {os} a VST3 bundle has another \
             layout, which is not built yet"
        )),
    }
}

/// Returns the bundle's folder under `root` and the path of the module in it.
pub fn layout(root: &Path, name: &str, arch: &str) -> (PathBuf, PathBuf) {
    let bundle = root.join(format!("{name}.vst3"));
    let module = bundle
        .join("Contents")
        .join(arch)
        .join(format!("{name}.vst3"));
    (bundle, module)
}

/// Returns the npm scripts that build a page, in order, from its
/// `package.json`: `typecheck` when the page has one, since Vite's build
/// strips types without checking them, then `build`.
pub fn page_scripts(package: &serde_json::Value) -> Vec<&'static str> {
    let has = |script: &str| package["scripts"][script].is_string();
    let check = has("typecheck").then_some("typecheck");
    check.into_iter().chain(["build"]).collect()
}

/// Says whether a page's packages need `npm ci`: when `node_modules` is
/// missing, or older than the lockfile. Each is the time its file was last
/// written; `node_modules` is dated by the lockfile `npm ci` writes into it.
pub fn needs_npm_ci(installed: Option<SystemTime>, lockfile: Option<SystemTime>) -> bool {
    match (installed, lockfile) {
        (None, _) => true,
        (Some(installed), Some(lockfile)) => installed < lockfile,
        (Some(_), None) => false,
    }
}

/// Words a failed copy into `folder` with: the host holding the module, the
/// folder needing an administrator, or the error itself.
pub fn copy_error(error: &io::Error, path: &Path, folder: Folder) -> String {
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

/// Runs `cargo xtask bundle` with the arguments after the task's name, and
/// returns 0, as a task that found nothing.
///
/// # Errors
///
/// Returns the step that failed and why.
pub fn run(args: impl IntoIterator<Item = String>) -> Result<usize, String> {
    let options = Options::parse(args)?;
    let arch = arch_folder(std::env::consts::OS, std::env::consts::ARCH)?;
    let root = crate::root();
    let metadata = cargo_metadata(&root)?;
    let (plugin, target) = plugin(&metadata, &options.package)?;
    let page = plugin.dir.join("page");
    if !page.join("package.json").is_file() {
        println!("{} has no page", options.package);
    } else if options.page {
        build_page(&page)?;
    } else {
        println!("skipped the page (--no-page): the bundle has the page last built");
    }
    run_tool(
        cargo()
            .args(["build", "--release", "-p", &options.package])
            .current_dir(&root),
        "cargo build",
    )?;
    let built = target.join("release").join(format!(
        "{}{}{}",
        std::env::consts::DLL_PREFIX,
        plugin.lib,
        std::env::consts::DLL_SUFFIX
    ));
    let (bundle, module) = layout(&target.join("bundle"), &plugin.name, arch);
    lay_out(&built, &bundle, &module)?;
    println!("bundled {}", bundle.display());
    if let Some(folder) = options.install {
        install(&bundle, &plugin.name, folder)?;
    }
    Ok(0)
}

fn cargo_metadata(root: &Path) -> Result<serde_json::Value, String> {
    let output = cargo()
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("cargo metadata: {e}"))
}

/// The Cargo that started this task, so a toolchain override holds.
fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

fn build_page(page: &Path) -> Result<(), String> {
    // npm is a script, `npm.cmd`, on Windows, and a bare name finds only an
    // `.exe` there.
    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
    let modified = |path: PathBuf| fs::metadata(path).and_then(|m| m.modified()).ok();
    let installed = modified(page.join("node_modules").join(".package-lock.json"));
    if needs_npm_ci(installed, modified(page.join("package-lock.json"))) {
        run_tool(Command::new(npm).arg("ci").current_dir(page), "npm ci")?;
    }
    let manifest = page.join("package.json");
    let package = fs::read(&manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let package =
        serde_json::from_slice(&package).map_err(|e| format!("{}: {e}", manifest.display()))?;
    for script in page_scripts(&package) {
        run_tool(
            Command::new(npm).args(["run", script]).current_dir(page),
            &format!("npm run {script}"),
        )?;
    }
    Ok(())
}

fn run_tool(command: &mut Command, what: &str) -> Result<(), String> {
    println!("running {what}");
    let status = command.status().map_err(|e| format!("{what}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{what} failed: {status}"))
    }
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

fn install(bundle: &Path, name: &str, folder: Folder) -> Result<(), String> {
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
