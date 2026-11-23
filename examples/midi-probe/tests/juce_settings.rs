//! The drift gate for the probe's JUCE settings: `hapto-juce.cmake` is the
//! probe's own `INFO`, generated. It is checked here rather than by `cargo
//! xtask gen`, since xtask linking a second plugin would bring two sets of
//! VST3 entry points into one binary. Rewrite it with `HAPTO_BLESS=1 cargo
//! test -p hapto-example-midi-probe --test juce_settings`.

use std::path::Path;

use hapto_example_midi_probe::MidiProbe;

/// The header's instruction for settings `cargo xtask gen` writes, and
/// these.
const XTASK: &str = "`cargo xtask gen`";
const HERE: &str = "`HAPTO_BLESS=1 cargo test -p hapto-example-midi-probe --test juce_settings`";

#[test]
fn the_juce_settings_match_the_probes_info() {
    let settings = hapto_host::cabi::juce_settings::<MidiProbe>(
        "examples/midi-probe/Cargo.toml and src/lib.rs (MidiProbe::INFO)",
    );
    assert!(
        settings.contains(XTASK),
        "the settings' header changed: update this test"
    );
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("hapto-juce.cmake");
    hapto_testkit::drift::check_generated(&path, &settings.replace(XTASK, HERE));
}
