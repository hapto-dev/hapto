//! `cargo hapto serve`: the page's dev server alone, until Ctrl+C, for a
//! debug bundle in a real host, which loads its page from it. A host keeps
//! the module loaded, so a change to Rust needs the bundle built again and
//! the host started again; the server goes on through both.

#[cfg(test)]
mod tests;

use crate::args::Args;
use crate::page::{DEV_PORT, DevServer, PAGE_DEV_URL};
use crate::workspace::Workspace;

/// What `cargo hapto help` prints for this command.
pub(crate) const USAGE: &str = "  serve [-p <crate>] [--port <n>]
                    serve the crate's page from its dev server until Ctrl+C, for its debug
                    bundle in a real host (cargo hapto build --debug --install): start the
                    host with HAPTO_PAGE_DEV_URL set to the address it prints, and the page
                    reloads live";

/// What `cargo hapto serve` was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Options {
    /// The plugin crate, by its package name; `None` finds it.
    pub(crate) package: Option<String>,
    /// The dev server's port.
    pub(crate) port: u16,
}

impl Options {
    /// Reads the command's arguments, its own name excluded.
    ///
    /// # Errors
    ///
    /// Returns what is wrong with them.
    pub(crate) fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
        let mut args = Args::new(args, USAGE);
        let mut options = Options {
            package: None,
            port: DEV_PORT,
        };
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-p" | "--package" => options.package = Some(args.value(&arg)?),
                "--port" => options.port = args.port(&arg)?,
                _ => return Err(args.unknown(&arg)),
            }
        }
        Ok(options)
    }
}

/// Runs `cargo hapto serve` with the arguments after the command's name,
/// until Ctrl+C ends it.
///
/// # Errors
///
/// Returns why the server did not start, or how it stopped on its own.
pub(crate) fn run_command(args: impl IntoIterator<Item = String>) -> Result<(), String> {
    let options = Options::parse(args)?;
    let (_, plugin) = Workspace::find(options.package.as_deref())?;
    let page = plugin
        .page()
        .ok_or_else(|| format!("{} has no page to serve", plugin.package))?;
    let server = DevServer::start(&page, options.port)?;
    println!(
        "the page is served from {url}: start the host with {PAGE_DEV_URL}={url} and \
         the debug bundle in it (cargo hapto build --debug --install), and the page reloads \
         live. Ctrl+C stops the server",
        url = server.url
    );
    Err(server.wait())
}
