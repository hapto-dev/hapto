//! A plugin's page in the host's window: a WebView, through wry, that loads
//! the page built into the plugin and joins it to the protocol's native
//! side.
//!
//! A plugin describes its page with [`Page`]: the files built into it, its
//! size, the values the page shows and writes, and the commands it may call.
//! [`Page::into_editor`] gives the
//! [`Editor`] that [`Plugin::editor`](seam_host::Plugin::editor) hands out,
//! and the exporter does the rest: it opens the editor in the host's window,
//! sizes it, ticks it after the plugin's idle, and closes it.
//!
//! What the editor does:
//!
//! - **The page's files** are served from the bytes built into the plugin,
//!   over a scheme of the editor's own, with their content types; any other
//!   path is answered 404. A debug build loads a dev server instead when
//!   `SEAM_PAGE_DEV_URL` is set, so the page's hot reload works in the
//!   plugin's window, with or without the files built in. A release build
//!   never reads it.
//! - **Messages.** What the page posts goes to a
//!   [`Dispatcher`](seam_bridge::Dispatcher) over the plugin's parameters, as
//!   it arrives; what the dispatcher posts reaches the page in order. The
//!   page's side is `webviewTransport` in the `seam-page` package. Each tick
//!   tells the page what changed.
//! - **Only the page.** The WebView navigates nowhere but the page, takes no
//!   message from any other document, and takes a file dropped on it from
//!   outside to nothing, where WebView2 would open it in the page's place.
//!   On Windows that also turns off HTML drag and drop inside the page.
//! - **Its data folder** is the plugin's own, below the user's local
//!   application data, named by the plugin's
//!   [`id`](seam_host::PluginInfo::id): the WebView's default, beside the
//!   host's executable, is not writable for a host below `Program Files`.
//!   Every instance of the plugin shares it, and so shares one WebView
//!   browser process.
//! - **Faults.** A WebView that does not start fails `open`, which is a fault
//!   of the instance with the WebView's reason: the host and the audio go on.
//!   wry leaves its window in the host's then, so the WebView is made hidden
//!   and shown once it started: a failed one shows nothing.
//!   The first time a page answers its `ready` after an open, one line says
//!   how long it took, where the fault lines go.
//!
//! The editor exists on Windows only, with WebView2; elsewhere
//! [`Page::into_editor`] gives `None`, and the host shows its own controls.
//! This crate is a shell: `seam-bridge` and the core crates have no window
//! or WebView dependency.
//!
//! # Examples
//!
//! A plugin's editor: its built page, a size, one value and one command.
//!
//! ```
//! use seam_host::{Editor, Plugin, Size};
//! use seam_params::Value;
//! use seam_webview::{Asset, Page};
//! # use seam_host::{AuCodes, Block, Category, Layout, Parts, PluginInfo, ProcessSetup, Processor, Status};
//! # struct Through;
//! # impl Processor for Through {
//! #     fn process(&mut self, _block: Block<'_, '_>) -> Status { Status::Continue }
//! # }
//!
//! /// The page's built files: `include_bytes!` of each in a real plugin.
//! const PAGE: &[Asset] = &[Asset { path: "index.html", bytes: b"<!doctype html>" }];
//!
//! struct Panel {
//!     parts: Parts,
//!     title: Value<String>,
//! }
//!
//! impl Plugin for Panel {
//! #   const INFO: PluginInfo = PluginInfo {
//! #       name: "Panel",
//! #       vendor: "Example",
//! #       url: "",
//! #       email: "",
//! #       description: "",
//! #       version: "0.1.0",
//! #       id: "dev.example.panel",
//! #       vst3_class_id: [0; 16],
//! #       au_codes: AuCodes { subtype: *b"panl", manufacturer: *b"Exmp" },
//! #       category: Category::Effect,
//! #       features: &[],
//! #       layouts: &[Layout::STEREO],
//! #       midi_input: false,
//! #       midi_output: false,
//! #   };
//!     type Params = ();
//!     type Processor = Through;
//!
//!     fn new(_params: (), parts: Parts) -> Self {
//!         let title = Value::new(String::from("Untitled"), parts.set());
//!         Panel { parts, title }
//!     }
//!
//!     fn activate(&mut self, _setup: &ProcessSetup) -> Through {
//!         Through
//!     }
//!
//!     fn editor(&mut self) -> Option<Box<dyn Editor>> {
//!         Page::new(&Self::INFO, self.parts.ui().clone(), PAGE)
//!             .with_size(Size { width: 480, height: 320 })
//!             .value("title", &self.title)
//!             .command("version", |()| Ok(Self::INFO.version))
//!             .into_editor()
//!     }
//! }
//!
//! let (params, parts) = Parts::for_plugin::<Panel>()?;
//! let mut panel = Panel::new(params, parts);
//! // An editor on Windows, none elsewhere.
//! assert_eq!(panel.editor().is_some(), cfg!(windows));
//! # Ok::<(), seam_params::DeclError>(())
//! ```

#[cfg_attr(
    not(windows),
    expect(dead_code, reason = "only the Windows editor serves the page")
)]
mod assets;
#[cfg(windows)]
mod windows;

use seam_bridge::{Commands, Values};
use seam_host::{Editor, PluginInfo, Size};
use seam_params::{UiParams, Value};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use assets::Asset;

/// A plugin's page: the files built into it, its size, the values it shows
/// and writes and the commands it may call, from which
/// [`into_editor`](Self::into_editor) makes the editor.
///
/// The [crate overview](crate) shows a plugin that hands one out.
///
/// # Examples
///
/// ```
/// # use seam_params::{Kind, ParamDecl, ParamSet};
/// use seam_host::{AuCodes, Category, Layout, PluginInfo, Size};
/// use seam_params::Value;
/// use seam_webview::{Asset, Page};
///
/// # const INFO: PluginInfo = PluginInfo {
/// #     name: "Panel", vendor: "Example", url: "", email: "", description: "",
/// #     version: "0.1.0", id: "dev.example.panel", vst3_class_id: [0; 16],
/// #     au_codes: AuCodes { subtype: *b"panl", manufacturer: *b"Exmp" },
/// #     category: Category::Effect, features: &[], layouts: &[Layout::STEREO],
/// #     midi_input: false, midi_output: false,
/// # };
/// # let set = ParamSet::from_decls([ParamDecl::new("gain", "Gain", Kind::Bool, 0.0)])?;
/// # let ui = set.take_ui().expect("the first UI handle");
/// const PAGE: &[Asset] = &[Asset { path: "index.html", bytes: b"<!doctype html>" }];
///
/// let name = Value::new(String::from("Untitled"), &set);
/// let editor = Page::new(&INFO, ui, PAGE)
///     .with_size(Size { width: 480, height: 320 })
///     .value("name", &name)
///     .command("greet", |name: String| Ok(format!("hello, {name}")))
///     .into_editor();
/// if let Some(editor) = editor {
///     assert_eq!(editor.size(), Size { width: 480, height: 320 });
/// }
/// # Ok::<(), seam_params::DeclError>(())
/// ```
#[cfg_attr(
    not(windows),
    expect(dead_code, reason = "only the Windows editor reads the page's files")
)]
pub struct Page {
    name: &'static str,
    id: &'static str,
    ui: UiParams,
    assets: &'static [Asset],
    size: Size,
    values: Values,
    commands: Commands,
}

impl Page {
    /// The size a page opens at unless [`with_size`](Self::with_size) gives
    /// one.
    pub const SIZE: Size = Size {
        width: 640,
        height: 400,
    };

    /// Describes the page of the plugin `info` names, writing to its
    /// parameters through `ui`, a clone of its parts' handle (`Parts::ui`),
    /// so a drag on the page is a user's, and served from `assets`, its
    /// built files. With none, only a debug build's dev server shows it.
    ///
    /// Thread: main. [`Page`] shows one.
    pub fn new(info: &PluginInfo, ui: UiParams, assets: &'static [Asset]) -> Self {
        Page {
            name: info.name,
            id: info.id,
            ui,
            assets,
            size: Self::SIZE,
            values: Values::new(),
            commands: Commands::new(),
        }
    }

    /// Sets the size it opens at, in logical pixels. The host may resize it
    /// from there.
    pub fn with_size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    /// Binds a value the page shows and writes with `useValue(key)`, as
    /// [`Values::value`] binds one: state that is not a parameter, shown as
    /// it is whatever changed it, and written by the page as the user.
    /// [`Page`] shows one.
    ///
    /// # Panics
    ///
    /// When `key` was bound already: give each value its own key.
    pub fn value<T>(mut self, key: &str, value: &Value<T>) -> Self
    where
        T: Clone + Serialize + DeserializeOwned + 'static,
    {
        self.values = self.values.value(key, value);
        self
    }

    /// Adds a command the page calls with `useCommand(name)`, as
    /// [`Commands::command`] adds one. [`Page`] shows one.
    ///
    /// # Panics
    ///
    /// When `name` was added already: give each command its own name.
    pub fn command<A, R>(
        mut self,
        name: &str,
        command: impl FnMut(A) -> Result<R, String> + 'static,
    ) -> Self
    where
        A: DeserializeOwned,
        R: Serialize,
    {
        self.commands = self.commands.command(name, command);
        self
    }

    /// Makes the editor that shows the page, for
    /// [`Plugin::editor`](seam_host::Plugin::editor) to hand out: `None` on
    /// a platform with no WebView here, where the host shows its own
    /// controls, and for a page with no files that no dev server serves.
    /// [`Page`] shows one.
    pub fn into_editor(self) -> Option<Box<dyn Editor>> {
        if self.assets.is_empty() && assets::dev_url().is_none() {
            return None;
        }
        #[cfg(windows)]
        {
            Some(Box::new(windows::PageEditor::new(self)))
        }
        #[cfg(not(windows))]
        {
            drop(self);
            None
        }
    }
}

impl std::fmt::Debug for Page {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Page")
            .field("plugin", &self.name)
            .field("size", &self.size)
            .field("values", &self.values)
            .field("commands", &self.commands)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use seam_host::{AuCodes, Category, Layout};
    use seam_params::{Kind, ParamDecl, ParamSet};

    use super::*;

    const INFO: PluginInfo = PluginInfo {
        name: "Panel",
        vendor: "Example",
        url: "",
        email: "",
        description: "",
        version: "0.1.0",
        id: "dev.example.panel",
        vst3_class_id: [0; 16],
        au_codes: AuCodes {
            subtype: *b"panl",
            manufacturer: *b"Exmp",
        },
        category: Category::Effect,
        features: &[],
        layouts: &[Layout::STEREO],
        midi_input: false,
        midi_output: false,
    };

    const PAGE: &[Asset] = &[Asset {
        path: "index.html",
        bytes: b"<!doctype html>",
    }];

    #[test]
    fn a_page_is_an_editor_on_windows_alone_at_its_size() {
        let set = ParamSet::from_decls([ParamDecl::new("gain", "Gain", Kind::Bool, 0.0)])
            .expect("valid declarations");
        let ui = set.take_ui().expect("the first UI handle");
        let page = Page::new(&INFO, ui, PAGE);
        assert_eq!(page.size, Page::SIZE);
        let size = Size {
            width: 300,
            height: 200,
        };
        let editor = page
            .with_size(size)
            .command("ping", |()| Ok(()))
            .into_editor();
        assert_eq!(editor.is_some(), cfg!(windows));
        if let Some(editor) = editor {
            assert_eq!(editor.size(), size, "nothing opens until the host attaches");
        }
    }

    #[test]
    fn a_page_with_no_files_is_an_editor_only_where_a_dev_server_serves_it() {
        let set = ParamSet::from_decls([ParamDecl::new("gain", "Gain", Kind::Bool, 0.0)])
            .expect("valid declarations");
        let ui = set.take_ui().expect("the first UI handle");
        let editor = Page::new(&INFO, ui, &[]).into_editor();
        // A debug build run with `SEAM_PAGE_DEV_URL` set has a dev server.
        let served = assets::dev_url().is_some();
        assert_eq!(editor.is_some(), cfg!(windows) && served);
    }

    #[test]
    #[should_panic(expected = "is bound twice")]
    fn a_value_bound_twice_panics() {
        let set = ParamSet::from_decls([ParamDecl::new("gain", "Gain", Kind::Bool, 0.0)])
            .expect("valid declarations");
        let ui = set.take_ui().expect("the first UI handle");
        let name = Value::new(String::new(), &set);
        let _ = Page::new(&INFO, ui, PAGE)
            .value("name", &name)
            .value("name", &name);
    }

    #[test]
    #[should_panic(expected = "is added twice")]
    fn a_command_added_twice_panics() {
        let set = ParamSet::from_decls([ParamDecl::new("gain", "Gain", Kind::Bool, 0.0)])
            .expect("valid declarations");
        let ui = set.take_ui().expect("the first UI handle");
        let _ = Page::new(&INFO, ui, PAGE)
            .command("ping", |()| Ok(()))
            .command("ping", |()| Ok(()));
    }
}
