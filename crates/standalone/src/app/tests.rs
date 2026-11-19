//! The app loads the session's saved block, and keeps a block the plugin
//! refused as it was, rather than save the plugin's defaults over it.

use std::path::PathBuf;

use hapto_host::standalone::Standalone;
use hapto_host::{
    AuCodes, Block, Category, Layout, Parts, Plugin, PluginInfo, ProcessSetup, Processor, Status,
};
use hapto_params::{Declarer, Params};

use super::load_saved;
use crate::session::Session;

#[derive(Clone)]
struct NoParams;

impl Params for NoParams {
    fn declare(_: &mut Declarer) -> Self {
        NoParams
    }
}

struct Through;

impl Processor for Through {
    fn process(&mut self, _block: Block<'_, '_>) -> Status {
        Status::Continue
    }
}

struct Quiet;

impl Plugin for Quiet {
    const INFO: PluginInfo = PluginInfo {
        name: "Quiet",
        vendor: "Test",
        url: "",
        email: "",
        description: "",
        version: "0.1.0",
        id: "dev.test.quiet",
        vst3_class_id: [2; 16],
        au_codes: AuCodes {
            subtype: *b"quie",
            manufacturer: *b"Test",
        },
        category: Category::Effect,
        features: &[],
        layouts: &[Layout::STEREO],
        midi_input: false,
        midi_output: false,
    };
    type Params = NoParams;
    type Processor = Through;

    fn new(_params: NoParams, _parts: Parts) -> Self {
        Quiet
    }

    fn activate(&mut self, _setup: &ProcessSetup) -> Through {
        Through
    }
}

/// A fresh folder of the test's own under the system's temporary folder,
/// holding `saved` as its saved block.
fn session_with(name: &str, saved: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("hapto-standalone-tests")
        .join(format!("app-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("made");
    std::fs::write(dir.join("saved.json"), saved).expect("written");
    dir
}

#[test]
fn a_block_the_plugin_refuses_is_kept_as_it_was() {
    let dir = session_with("refused", b"not a block");
    let mut session = Session::in_dir(dir.clone()).expect("opened");
    let plugin = Standalone::<Quiet>::new();
    load_saved(&mut session, &plugin);
    // As the app keeps the block each second, and when it quits.
    session.keep(plugin.save()).expect("nothing to write");
    assert_eq!(
        std::fs::read(dir.join("saved.json")).expect("read"),
        b"not a block"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_block_the_plugin_takes_is_kept_as_it_changes() {
    let own = Standalone::<Quiet>::new().save();
    let dir = session_with("taken", &own);
    let mut session = Session::in_dir(dir.clone()).expect("opened");
    let plugin = Standalone::<Quiet>::new();
    load_saved(&mut session, &plugin);
    session.keep(b"{}".to_vec()).expect("written");
    assert_eq!(std::fs::read(dir.join("saved.json")).expect("read"), b"{}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_minimized_window_s_size_is_none_to_hand_an_editor() {
    use winit::dpi::PhysicalSize;
    assert!(
        super::minimized(PhysicalSize::new(0, 0)),
        "as winit gives it"
    );
    assert!(super::minimized(PhysicalSize::new(0, 480)));
    assert!(!super::minimized(PhysicalSize::new(800, 600)));
}

#[test]
fn the_window_s_title_says_what_it_shows_of_the_editor() {
    use super::{EditorShown, title};
    assert_eq!(title("Gain", EditorShown::Open), "Gain");
    assert_eq!(title("Gain", EditorShown::None), "Gain (no editor)");
    assert_eq!(
        title("Gain", EditorShown::Failed),
        "Gain (its editor did not open)"
    );
}
