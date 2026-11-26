//! What differs between the systems that the editor runs on. Each system's
//! web engine implements [`WebEngine`], and cfg picks the one that this build
//! uses as [`Engine`].

use std::path::PathBuf;

use hapto_host::{ParentWindow, Size};

use crate::Asset;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
pub use linux::set_helper;

#[cfg(target_os = "linux")]
pub(crate) use linux::WebKitGtk as Engine;
#[cfg(windows)]
pub(crate) use windows::WebView2 as Engine;

/// The page, as a system's web engine shows it in the host's window.
/// Dropping it removes everything that it put there.
///
/// Thread: main, for each item.
pub(crate) trait WebEngine: Sized {
    /// The name of the engine's own folder in the plugin's data folder, where
    /// the engine keeps its data.
    const FOLDER: &'static str;

    /// Opens the page that `load` describes inside `parent`, sized to `size`
    /// logical pixels at the host's `scale`. In a release build the engine
    /// must turn off the browser's context menu and its shortcuts, such as a
    /// reload, wherever it has them.
    ///
    /// # Errors
    ///
    /// Returns why the page didn't open, such as a WebView that didn't start
    /// or a `parent` that the engine can't show a page in. Nothing of the
    /// page is left in `parent` then.
    fn open(load: Load, parent: &ParentWindow, size: Size, scale: f64) -> Result<Self, String>;

    /// Returns a folder for the engine's data when the user's data folder is
    /// unknown, or `None` when the engine has none. The folder must belong to
    /// the user, because it holds the page's storage.
    fn fallback() -> Option<PathBuf>;

    /// Runs `script` in the page. A script that the page can't run is lost,
    /// and the page gets the current state again in the snapshot that answers
    /// its next `ready`.
    fn evaluate(&self, script: &str);

    /// Sizes the page to `size` logical pixels at the host's `scale`, at the
    /// top left of the host's window. A page whose window the host destroyed
    /// isn't placed, and the close that follows drops it.
    fn place(&self, size: Size, scale: f64);

    /// Gives the keyboard to the host's window.
    ///
    /// # Errors
    ///
    /// Returns why the engine couldn't focus the host's window.
    fn focus_host(&self) -> Result<(), String>;

    /// Passes on each message that the page posted since the last call. The
    /// editor calls it at every tick. Returns why the page has gone, at the
    /// first call after the engine lost it.
    ///
    /// An engine also passes on, as its view drops, the messages that the
    /// page posted before it closed.
    fn poll(&self) -> Option<String>;
}

/// What the editor gives an engine to open the page with.
pub(crate) struct Load {
    /// The dev server's address, when the page loads from it.
    pub(crate) dev: Option<String>,
    /// The engine's data folder, which exists.
    pub(crate) folder: PathBuf,
    /// The page's files, which the engine serves at the page's scheme.
    pub(crate) assets: &'static [Asset],
    /// Called with each message that the page posts. The engine drops the
    /// messages of any other document.
    pub(crate) receive: Box<dyn Fn(String)>,
    /// Whether the page keeps the browser's menu and its inspector, as a debug
    /// build of the plugin does.
    pub(crate) devtools: bool,
    /// Whether WebKitGTK may draw the page with the GPU, as
    /// [`Page::with_hardware_acceleration`][gpu] asks.
    ///
    /// [gpu]: crate::Page::with_hardware_acceleration
    #[cfg(target_os = "linux")]
    pub(crate) accelerated: bool,
}
