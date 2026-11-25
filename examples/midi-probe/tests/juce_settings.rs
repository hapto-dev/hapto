//! The drift gate for `hapto-juce.cmake`, the probe's JUCE settings generated
//! from its `INFO`.
//!
//! It is checked here, not by `cargo xtask gen`, because xtask linking a
//! second plugin would put two sets of VST3 entry points in one binary.
//! Rewrite it with `cargo hapto juce -p hapto-example-midi-probe`.

use std::path::Path;

use hapto_example_midi_probe::MidiProbe;

/// The header's instruction for settings that `cargo xtask gen` writes, and
/// (`HERE`) for these.
const XTASK: &str = "`cargo xtask gen`";
const HERE: &str = "`cargo hapto juce -p hapto-example-midi-probe`";

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
