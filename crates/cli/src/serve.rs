//! `cargo hapto serve`: the page's dev server alone, until Ctrl+C, for a
//! debug bundle in a real host, which loads its page from it. A host keeps
//! the module loaded, so a change to Rust needs the bundle built again and
//! the host started again; the server goes on through both.

#[cfg(test)]
mod tests;

use anyhow::{Context as _, Result};

use crate::page::{DEV_PORT, DevServer, PAGE_DEV_URL};
use crate::workspace::Workspace;

/// What `cargo hapto serve` was asked to do.
#[derive(Clone, Debug, PartialEq, Eq, clap::Args)]
pub(crate) struct Options {
    /// The plugin crate, by its package name; else the one the folder is
    /// in, or the workspace's only one.
    #[arg(short, long, value_name = "CRATE")]
    pub(crate) package: Option<String>,
    /// The dev server's port, on 127.0.0.1.
    #[arg(long, default_value_t = DEV_PORT, value_parser = clap::value_parser!(u16).range(1..))]
    pub(crate) port: u16,
}

/// Runs `cargo hapto serve` with `options`, until Ctrl+C ends it.
///
/// # Errors
///
/// Returns why the server did not start, or how it stopped on its own.
pub(crate) fn run(options: &Options) -> Result<()> {
    let (_, plugin) = Workspace::find(options.package.as_deref())?;
    let page = plugin
        .page()
        .with_context(|| format!("{} has no page to serve", plugin.package))?;
    let server = DevServer::start(&page, options.port)?;
    println!(
        "the page is served from {url}: start the host with {PAGE_DEV_URL}={url} and \
         the debug bundle in it (cargo hapto build --debug --install), and the page reloads \
         live. Ctrl+C stops the server",
        url = server.url
    );
    Err(server.wait())
}
