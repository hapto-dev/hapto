//! The values that the crate's unit tests share. They are a plugin's info, a
//! page of one file, one parameter, and a `dialog` command that runs code
//! while the command is in a call.

use std::cell::RefCell;
use std::sync::Arc;

use hapto_host::{AuCodes, Category, PluginInfo};
use hapto_params::{Kind, ParamDecl, ParamId, ParamSet, UiParams};

use crate::Asset;

/// A plugin's info, for the page's editor.
pub(crate) const INFO: PluginInfo = PluginInfo {
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
    layouts: &[],
    midi_input: false,
    midi_output: false,
};

/// A page of one file.
pub(crate) const PAGE: &[Asset] = &[Asset {
    path: "index.html",
    bytes: b"<!doctype html>",
}];

/// The page's first message, which asks for a snapshot.
pub(crate) const READY: &str = r#"{"type":"ready","protocol":3}"#;

/// The page's call of the `dialog` command.
pub(crate) const DIALOG: &str = r#"{"type":"invoke","call":1,"command":"dialog","args":null}"#;

/// Returns a set of one parameter, `bypass`, with the set's UI handle taken
/// out of it, and the parameter's id.
pub(crate) fn params() -> (Arc<ParamSet>, UiParams, ParamId) {
    let set = ParamSet::from_decls([ParamDecl::new("bypass", "Bypass", Kind::Bool, 0.0)])
        .expect("valid declarations");
    let bypass = set.id("bypass").expect("declared");
    let ui = set.take_ui().expect("the first UI handle");
    (set, ui, bypass)
}

thread_local! {
    /// What arrives while the page's `dialog` command runs, as a file
    /// dialog's message loop would deliver it.
    static DURING_DIALOG: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

/// Makes the page's `dialog` command run `during`, on this thread.
pub(crate) fn during_dialog(during: impl Fn() + 'static) {
    DURING_DIALOG.with(|slot| *slot.borrow_mut() = Some(Box::new(during)));
}

/// The page's `dialog` command, which runs what [`during_dialog`] gave.
pub(crate) fn dialog(_: ()) -> Result<(), String> {
    DURING_DIALOG.with(|during| {
        if let Some(during) = &*during.borrow() {
            during();
        }
    });
    Ok(())
}
