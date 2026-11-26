//! A plugin's page in the host's window: a WebView, through wry, that loads
//! the page built into the plugin and joins it to the protocol's native
//! side.
//!
//! A plugin describes its page with [`Page`]: the files built into it, its
//! size, the values the page shows and writes, and the commands it may call.
//! [`Page::into_editor`] gives the
//! [`Editor`] that [`Plugin::editor`](hapto_host::Plugin::editor) hands out,
//! and the exporter does the rest: it opens the editor in the host's window,
//! sizes it, ticks it after the plugin's idle, and closes it.
//!
//! What the editor does:
//!
//! - **The page's files** are served from the bytes built into the plugin,
//!   over a scheme of the editor's own, with their content types; any other
//!   path is answered 404. A debug build loads a dev server instead when
//!   `HAPTO_PAGE_DEV_URL` is set, so the page's hot reload works in the
//!   plugin's window, with or without the files built in. A release build
//!   never reads it.
//! - **Messages.** What the page posts goes to a
//!   [`Dispatcher`](hapto_bridge::Dispatcher) over the plugin's parameters, as
//!   it arrives; what the dispatcher posts reaches the page in order. The
//!   page's side is `webviewTransport` in the `@hapto/page` package. Each tick
//!   tells the page what changed.
//! - **Only the page.** The WebView navigates nowhere but the page, takes no
//!   message from any other document, and takes a file dropped on it from
//!   outside to nothing, where WebView2 would open it in the page's place.
//!   On Windows that also turns off HTML drag and drop inside the page.
//! - **The keyboard.** The WebView takes it when the page is clicked, not as
//!   it opens. Whenever nothing in the page needs keys, after a click, a drag
//!   or a field's blur, the page's `webviewTransport` asks, and the editor
//!   gives the keyboard back to the host's window, so a key such as the space
//!   bar reaches the host again. A field, a `select` and a control marked
//!   `data-hapto-keys` keep it while they have the focus. On Linux, X sends a
//!   key to the window under the pointer, so even while the page holds the
//!   keyboard, a key pressed with the pointer outside the page reaches the
//!   host. The ask is not a protocol message, and the dispatcher never sees
//!   it.
//! - **Its data folder** is the engine's own folder inside the plugin's data
//!   folder, which is named by the plugin's
//!   [`id`](hapto_host::PluginInfo::id). The editor sets it, because a host
//!   below `Program Files` can't write WebView2's default folder, beside the
//!   host's executable. Every instance of the plugin shares it.
//! - **Faults.** A WebView that does not start fails `open`, which is a fault
//!   of the instance with the WebView's reason, and the host and the audio go
//!   on. The WebView is made hidden and shown once it started, so a failed one
//!   shows nothing in the host's window.
//!   The first time a page answers its `ready` after an open, one line says
//!   how long it took, where the fault lines go, and one line says when an
//!   open page is gone, as when its helper process on Linux exits.
//!
//! The editor exists on Windows, with WebView2, and on Linux, with WebKitGTK in
//! an X11 window. On Linux WebKitGTK runs in a helper process, one for each
//! plugin in each host process, so the host's process loads no GTK. This crate
//! builds the helper, `hapto-webview-helper`, with its `helper` feature, and
//! the editor runs it from the folder that holds the plugin's module. Open: the
//! plugin's bundle doesn't install it there yet. Elsewhere
//! [`Page::into_editor`] gives `None`, and the host shows its own controls.
//! This crate is a shell: `hapto-bridge` and the core crates have no window
//! or WebView dependency.
//!
//! # Examples
//!
//! A plugin's editor: its built page, a size, one value and one command.
//!
//! ```
//! use hapto_host::{Editor, Plugin, Size};
//! use hapto_params::Value;
//! use hapto_webview::{Asset, Page};
//! # use hapto_host::layout::Stereo;
//! # use hapto_host::{AuCodes, Block, Category, Parts, PluginInfo, ProcessSetup, Processor, Status};
//! # struct Through;
//! # impl Processor for Through {
//! #     type Layout = Stereo;
//! #     fn process(&mut self, _block: Block<'_, Stereo>) -> Status { Status::Continue }
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
//! #       layouts: &[],
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
//! // An editor on Windows and Linux, none elsewhere.
//! assert_eq!(
//!     panel.editor().is_some(),
//!     cfg!(any(windows, target_os = "linux"))
//! );
//! # Ok::<(), hapto_params::DeclError>(())
//! ```

#[cfg_attr(
    not(any(test, hapto_wry)),
    expect(dead_code, reason = "only the code that runs wry serves the page")
)]
mod assets;
#[cfg(hapto_webview)]
mod editor;
#[cfg(test)]
mod fixtures;
#[cfg(any(hapto_webview, test))]
mod native;
#[cfg(hapto_webview)]
mod platform;
#[cfg(any(test, hapto_helper, hapto_webkitgtk))]
mod wire;
#[cfg(hapto_wry)]
mod wry_page;

#[cfg(hapto_helper)]
#[doc(hidden)]
pub mod helper;
#[cfg(hapto_webkitgtk)]
#[doc(hidden)]
pub use platform::set_helper;
/// The page's helper program's name, for a test that checks it against the
/// program that Cargo builds.
#[cfg(hapto_webkitgtk)]
#[doc(hidden)]
pub const HELPER: &str = wire::HELPER;

use hapto_bridge::{Commands, Values};
use hapto_host::{Editor, PluginInfo, Size};
use hapto_params::{UiParams, Value};
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
/// # use hapto_params::{Kind, ParamDecl, ParamSet};
/// use hapto_host::{AuCodes, Category, PluginInfo, Size};
/// use hapto_params::Value;
/// use hapto_webview::{Asset, Page};
///
/// # const INFO: PluginInfo = PluginInfo {
/// #     name: "Panel", vendor: "Example", url: "", email: "", description: "",
/// #     version: "0.1.0", id: "dev.example.panel", vst3_class_id: [0; 16],
/// #     au_codes: AuCodes { subtype: *b"panl", manufacturer: *b"Exmp" },
/// #     category: Category::Effect, features: &[], layouts: &[],
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
/// # Ok::<(), hapto_params::DeclError>(())
/// ```
#[cfg_attr(
    not(hapto_webview),
    expect(dead_code, reason = "only the editor reads the page's files")
)]
pub struct Page {
    name: &'static str,
    id: &'static str,
    ui: UiParams,
    assets: &'static [Asset],
    size: Size,
    values: Values,
    commands: Commands,
    /// Whether WebKitGTK may draw the page with the GPU.
    accelerated: bool,
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
            accelerated: false,
        }
    }

    /// Sets the size it opens at, in logical pixels. The host may resize it
    /// from there.
    pub fn with_size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    /// Lets WebKitGTK draw the page with the GPU on Linux.
    ///
    /// Without this call, WebKitGTK draws the page in software, because its
    /// GPU path shows a blank page on some systems, such as virtual machines
    /// and some with NVIDIA's drivers. Call it for a page that needs the GPU,
    /// such as one that uses WebGL, and test the page on the systems that its
    /// users run. The first of the plugin's pages to open in a host's process
    /// sets it for all of them there, because one helper process shows them
    /// all. WebView2 on Windows always draws with the GPU.
    pub fn with_hardware_acceleration(mut self) -> Self {
        self.accelerated = true;
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
    /// [`Plugin::editor`](hapto_host::Plugin::editor) to hand out: `None` on
    /// a platform with no WebView here, where the host shows its own
    /// controls, and for a page with no files that no dev server serves.
    /// [`Page`] shows one.
    pub fn into_editor(self) -> Option<Box<dyn Editor>> {
        if self.assets.is_empty() && assets::dev_url().is_none() {
            return None;
        }
        #[cfg(hapto_webview)]
        {
            Some(Box::new(editor::PageEditor::new(self)))
        }
        #[cfg(not(hapto_webview))]
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
            .field("accelerated", &self.accelerated)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{INFO, PAGE, params};

    #[test]
    fn the_dependency_tables_name_the_systems_that_the_build_script_lists() {
        hapto_testkit::systems::assert_target_tables_match(
            include_str!("../Cargo.toml"),
            env!("HAPTO_WEBVIEW_SYSTEMS"),
        );
    }

    #[test]
    fn a_page_is_an_editor_at_its_size_where_a_web_engine_exists() {
        let (_, ui, _) = params();
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
        assert_eq!(editor.is_some(), cfg!(hapto_webview));
        if let Some(editor) = editor {
            assert_eq!(editor.size(), size, "nothing opens until the host attaches");
        }
    }

    #[test]
    fn a_page_with_no_files_is_an_editor_only_where_a_dev_server_serves_it() {
        let (_, ui, _) = params();
        let editor = Page::new(&INFO, ui, &[]).into_editor();
        // A debug build run with `HAPTO_PAGE_DEV_URL` set has a dev server.
        let served = assets::dev_url().is_some();
        assert_eq!(editor.is_some(), cfg!(hapto_webview) && served);
    }

    #[test]
    #[should_panic(expected = "is bound twice")]
    fn a_value_bound_twice_panics() {
        let (set, ui, _) = params();
        let name = Value::new(String::new(), &set);
        let _ = Page::new(&INFO, ui, PAGE)
            .value("name", &name)
            .value("name", &name);
    }

    #[test]
    #[should_panic(expected = "is added twice")]
    fn a_command_added_twice_panics() {
        let (_, ui, _) = params();
        let _ = Page::new(&INFO, ui, PAGE)
            .command("ping", |()| Ok(()))
            .command("ping", |()| Ok(()));
    }
}
