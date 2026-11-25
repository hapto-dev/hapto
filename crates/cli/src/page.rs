//! A plugin's page, the npm package in its `page` folder: its packages, its
//! check and build, and its dev server.
//!
//! Cargo never runs Node, so the commands do: a page that does not check, or
//! a missing or stale build, never reaches a bundle unseen.

#[cfg(test)]
mod tests;

use std::ffi::OsStr;
use std::fs;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context as _, Result, anyhow, bail};

use crate::tool::{generating_test, npm, run};
use crate::workspace::{Plugin, Workspace};

/// The port the dev server listens on unless `--port` names another.
pub(crate) const DEV_PORT: u16 = 5173;

/// How long the dev server has to start listening.
const DEV_START: Duration = Duration::from_secs(20);

/// Writes the page's parameter map from the plugin's declarations, through
/// its `page_params` test, when it has a page and that test, or in CI checks
/// it: a plugin's map is its own test's to write, since a tool that linked
/// the plugin would carry its entry points.
///
/// # Errors
///
/// Returns the test's failure: a declaration it refused, or in CI a map that
/// does not match the declarations.
pub(crate) fn write_param_map(workspace: &Workspace, plugin: &Plugin) -> Result<()> {
    if !plugin.has_param_map_test || plugin.page().is_none() {
        return Ok(());
    }
    let ci = std::env::var_os("CI");
    run(
        &mut param_map_test(&plugin.package, &workspace.root, ci.as_deref()),
        "the page's parameter map",
    )
}

/// Returns the `page_params` test of `package`, run from `root` by
/// [`generating_test`] with `ci` as the value of `CI`.
fn param_map_test(package: &str, root: &Path, ci: Option<&OsStr>) -> Command {
    generating_test(package, "page_params", root, ci)
}

/// Writes `plugin`'s parameter map and builds its page, in `page`, as a
/// build that embeds the page does first.
///
/// # Errors
///
/// Returns the step that failed.
pub(crate) fn prepare(workspace: &Workspace, plugin: &Plugin, page: &Path) -> Result<()> {
    write_param_map(workspace, plugin)?;
    build(page)
}

/// Installs the page's packages when they are missing or stale, then
/// type-checks it when it has a `typecheck` script, and builds it.
///
/// # Errors
///
/// Returns the step that failed.
pub(crate) fn build(page: &Path) -> Result<()> {
    install(page)?;
    let package = read_package(page)?;
    // TypeScript checks a linked package's source with that package's own
    // packages, which a fresh checkout has not installed.
    for linked in linked(page, &package) {
        if !linked.join("node_modules").is_dir() {
            install(&linked)?;
        }
    }
    for script in scripts(&package) {
        run(
            npm().args(["run", script]).current_dir(page),
            &format!("npm run {script}"),
        )?;
    }
    Ok(())
}

/// Installs the page's packages when they are missing or stale: `npm ci`
/// from its lockfile, or `npm install`, which writes one, for a page that
/// has none yet, as one `cargo hapto init` just wrote.
///
/// # Errors
///
/// Returns the npm step that failed.
pub(crate) fn install(page: &Path) -> Result<()> {
    let modified = |path: PathBuf| fs::metadata(path).and_then(|m| m.modified()).ok();
    let installed = modified(page.join("node_modules").join(".package-lock.json"));
    let lockfile = modified(page.join("package-lock.json"));
    match packages(installed, lockfile) {
        Packages::Current => Ok(()),
        Packages::FromLockfile => run(npm().arg("ci").current_dir(page), "npm ci"),
        Packages::Resolve => run(npm().arg("install").current_dir(page), "npm install"),
    }
}

/// What a page's packages need before a build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Packages {
    /// Nothing: installed since the lockfile was last written.
    Current,
    /// `npm ci`: missing, or older than the lockfile.
    FromLockfile,
    /// `npm install`: there is no lockfile to install from.
    Resolve,
}

/// Says what a page's packages need, from when `node_modules` and the
/// lockfile were last written. `node_modules` is dated by the lockfile npm
/// writes into it.
pub(crate) fn packages(installed: Option<SystemTime>, lockfile: Option<SystemTime>) -> Packages {
    match (installed, lockfile) {
        (_, None) => Packages::Resolve,
        (None, Some(_)) => Packages::FromLockfile,
        (Some(installed), Some(lockfile)) if installed < lockfile => Packages::FromLockfile,
        (Some(_), Some(_)) => Packages::Current,
    }
}

/// Returns the npm scripts that build a page, in order, from its
/// `package.json`: `typecheck` when the page has one, since Vite's build
/// strips types without checking them, then `build`.
pub(crate) fn scripts(package: &serde_json::Value) -> Vec<&'static str> {
    let has = |script: &str| package["scripts"][script].is_string();
    let check = has("typecheck").then_some("typecheck");
    check.into_iter().chain(["build"]).collect()
}

/// Returns the folder of each package the page takes by `file:`, such as
/// `@hapto/page` from a checkout, that holds a `package.json`.
pub(crate) fn linked(page: &Path, package: &serde_json::Value) -> Vec<PathBuf> {
    ["dependencies", "devDependencies"]
        .iter()
        .filter_map(|table| package[table].as_object())
        .flat_map(|table| table.values())
        .filter_map(|spec| spec.as_str()?.strip_prefix("file:"))
        .map(|path| page.join(path))
        .filter(|dir| dir.join("package.json").is_file())
        .collect()
}

fn read_package(page: &Path) -> Result<serde_json::Value> {
    let manifest = page.join("package.json");
    let bytes = fs::read(&manifest).with_context(|| manifest.display().to_string())?;
    serde_json::from_slice(&bytes).with_context(|| manifest.display().to_string())
}

/// The variable a debug build reads its page's dev server from:
/// `hapto-webview`'s own, which a test reads.
pub(crate) const PAGE_DEV_URL: &str = "HAPTO_PAGE_DEV_URL";

/// The page's dev server, Vite, running until dropped.
#[derive(Debug)]
pub(crate) struct DevServer {
    child: Child,
    /// The address a debug build loads the page from.
    pub(crate) url: String,
}

/// Refuses `port` when something on the loopback address listens on it
/// already: the start's check would take that for Vite, which `--strictPort`
/// ends, and the plugin would load another project's page.
fn claim(port: u16) -> Result<()> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .map(drop)
        .map_err(|e| {
            anyhow!(
                "port {port} on 127.0.0.1 is taken ({e}): another dev server holds it; \
                 stop that one, or pass --port"
            )
        })
}

impl DevServer {
    /// Starts Vite in `page` on `port` of the loopback address, which it must
    /// have: a server on another port would leave the plugin loading none.
    /// Returns once it listens.
    ///
    /// Vite runs straight from Node, not through `npm run dev`, so the
    /// process this holds is the server's own: on Windows npm is a script
    /// whose child would outlive a kill.
    ///
    /// # Errors
    ///
    /// Returns why it did not start: no Vite in the page's packages, no
    /// Node, or no server listening in time.
    pub(crate) fn start(page: &Path, port: u16) -> Result<DevServer> {
        // First, so a port held already is refused before an install that
        // can take minutes.
        claim(port)?;
        install(page)?;
        let vite = page
            .join("node_modules")
            .join("vite")
            .join("bin")
            .join("vite.js");
        if !vite.is_file() {
            bail!(
                "{} has no Vite in its packages, and the dev server is Vite",
                page.display()
            );
        }
        let child = Command::new("node")
            .arg(&vite)
            .args([
                "--host",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--strictPort",
            ])
            .current_dir(page)
            .stdin(Stdio::null())
            .spawn()
            .context("node, for the page's dev server")?;
        let mut server = DevServer {
            child,
            url: format!("http://127.0.0.1:{port}"),
        };
        server.wait_until_listening(port)?;
        Ok(server)
    }

    fn wait_until_listening(&mut self, port: u16) -> Result<()> {
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let end = Instant::now() + DEV_START;
        while Instant::now() < end {
            if let Ok(Some(status)) = self.child.try_wait() {
                bail!("the page's dev server stopped: {status}");
            }
            if TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_ok() {
                // What answered is this server only while it runs.
                return match self.child.try_wait() {
                    Ok(Some(status)) => Err(anyhow!("the page's dev server stopped: {status}")),
                    _ => Ok(()),
                };
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(anyhow!(
            "the page's dev server did not listen on {address} within {} seconds",
            DEV_START.as_secs()
        ))
    }

    /// Waits until the server stops, which only a fault or Ctrl+C does, and
    /// returns how it stopped. Ctrl+C ends this command with it.
    pub(crate) fn wait(mut self) -> anyhow::Error {
        match self.child.wait() {
            Ok(status) => anyhow!("the page's dev server stopped: {status}"),
            Err(e) => anyhow::Error::new(e).context("the page's dev server"),
        }
    }
}

impl Drop for DevServer {
    fn drop(&mut self) {
        // Already gone when it stopped on its own, which the kill reports.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
