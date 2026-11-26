use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::{cmake, settings_test};
use crate::workspace::Plugin;

#[test]
fn the_settings_test_is_run_quiet_and_writes_unless_in_ci() {
    let command = settings_test("probe", Path::new("."), None);
    let args: Vec<&OsStr> = command.get_args().collect();
    let expected = ["test", "-p", "probe", "--test", "juce_settings", "--", "-q"];
    assert_eq!(args, expected.map(OsStr::new));
    let blesses = |ci: Option<&str>| {
        settings_test("probe", Path::new("."), ci.map(OsStr::new))
            .get_envs()
            .any(|(key, value)| key == "HAPTO_BLESS" && value == Some(OsStr::new("1")))
    };
    assert!(blesses(None), "written where CI is not set");
    assert!(!blesses(Some("true")), "checked in CI");
}

#[test]
fn the_cmake_names_the_package_its_settings_and_its_workspace() {
    let plugin = Plugin {
        package: "my-synth".to_owned(),
        name: "My Synth 2".to_owned(),
        lib: "my_synth".to_owned(),
        dir: PathBuf::from("/work/plugins/my-synth"),
        standalone: None,
        has_param_map_test: false,
        has_juce_settings_test: true,
    };
    let text = cmake(
        &plugin,
        Path::new("/work"),
        "/hapto/adapters/juce/HaptoJuce.cmake",
    );
    assert!(
        text.contains("include(\"/hapto/adapters/juce/HaptoJuce.cmake\")"),
        "{text}"
    );
    assert!(text.contains("hapto_add_juce_plugin(MySynth2\n"), "{text}");
    assert!(text.contains("PACKAGE my-synth\n"), "{text}");
    assert!(
        text.contains("SETTINGS \"/work/plugins/my-synth/hapto-juce.cmake\""),
        "{text}"
    );
    assert!(
        text.contains("MANIFEST_PATH \"/work/Cargo.toml\""),
        "{text}"
    );
}
