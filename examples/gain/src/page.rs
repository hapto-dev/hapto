//! The gain's page: a slider on the gain, a field for the label and the
//! plugin's version, in the host's window. Its source is `page/`, a Vite and
//! React app; `npm run build` there writes the files below, which the plugin
//! is built with. A build that finds them missing says so, and the plugin
//! offers no editor, unless a debug build loads the page from a dev server.

use hapto_host::{Editor, Plugin, Size};
use hapto_webview::{Asset, Page};

use crate::Gain;

/// The page's built files, when they were there to build in.
#[cfg(hapto_page)]
pub const PAGE: Option<&[Asset]> = Some(&[
    Asset {
        path: "index.html",
        bytes: include_bytes!("../page/dist/index.html"),
    },
    Asset {
        path: "assets/index.js",
        bytes: include_bytes!("../page/dist/assets/index.js"),
    },
    Asset {
        path: "assets/index.css",
        bytes: include_bytes!("../page/dist/assets/index.css"),
    },
]);

/// The page's built files, when they were there to build in.
#[cfg(not(hapto_page))]
pub const PAGE: Option<&[Asset]> = None;

/// The editor that shows the page: none when the page was not built in and
/// no dev server serves it.
/// The page binds the value the plugin saves, so it shows the label a load or
/// an undo brings, and a rename from the page is the user's write, which undo
/// records and the plugin saves.
pub(crate) fn editor(gain: &Gain) -> Option<Box<dyn Editor>> {
    Page::new(&Gain::INFO, gain.ui().clone(), PAGE.unwrap_or_default())
        .with_size(Size {
            width: 420,
            height: 220,
        })
        .value("label", &gain.label)
        .command("version", |()| Ok(Gain::INFO.version))
        .into_editor()
}
