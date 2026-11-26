//! The workspace is read from `cargo metadata`, and a command finds its
//! plugin by name, by the folder it started in, or as the only one.

use std::path::{Path, PathBuf};

use super::{Plugin, Workspace, locked_version};

fn package(name: &str, dir: &str, kinds: &[&str], hapto: Option<&str>) -> serde_json::Value {
    let mut package = serde_json::json!({
        "name": name,
        "manifest_path": format!("{dir}/Cargo.toml"),
        "targets": [
            { "name": "guide", "kind": ["test"] },
            { "name": name, "kind": kinds },
        ],
        "metadata": null,
    });
    if let Some(name) = hapto {
        package["metadata"] = serde_json::json!({ "hapto": { "name": name } });
    }
    package
}

fn workspace(packages: &[serde_json::Value]) -> Workspace {
    Workspace::from_metadata(&serde_json::json!({
        "workspace_root": "/w",
        "target_directory": "/w/target",
        "packages": packages,
    }))
    .expect("valid metadata")
}

fn gain() -> serde_json::Value {
    package(
        "hapto-example-gain",
        "/w/examples/gain",
        &["cdylib", "rlib"],
        Some("Hapto Example Gain"),
    )
}

fn eq() -> serde_json::Value {
    package(
        "hapto-example-eq",
        "/w/examples/eq",
        &["cdylib", "rlib"],
        Some("Hapto Example EQ"),
    )
}

#[test]
fn a_plugin_crate_is_read_from_the_metadata() {
    let workspace = workspace(&[gain()]);
    assert_eq!(workspace.root, Path::new("/w"));
    assert_eq!(workspace.target, Path::new("/w/target"));
    let found = workspace
        .plugin(Some("hapto-example-gain"), Path::new("/elsewhere"))
        .expect("a plugin");
    assert_eq!(
        found,
        Plugin {
            package: "hapto-example-gain".into(),
            name: "Hapto Example Gain".into(),
            lib: "hapto_example_gain".into(),
            dir: PathBuf::from("/w/examples/gain"),
            standalone: None,
            has_param_map_test: false,
            has_juce_settings_test: false,
        }
    );
}

#[test]
fn a_crate_that_is_no_plugin_is_refused_by_name() {
    let tool = package("tool", "/w/tool", &["bin"], None);
    let no_name = package("no-name", "/w/no-name", &["cdylib"], None);
    let workspace = workspace(&[gain(), tool, no_name]);
    let dir = Path::new("/w");
    let refused = |name| {
        format!(
            "{:#}",
            workspace.plugin(Some(name), dir).expect_err("refused")
        )
    };
    assert!(refused("hapto-example-eq").contains("not a crate of this workspace"));
    assert!(refused("tool").contains("no cdylib"));
    assert!(refused("no-name").contains("[package.metadata.hapto]"));
    assert!(Workspace::from_metadata(&serde_json::json!({})).is_err());
}

#[test]
fn with_no_name_the_plugin_is_the_one_the_folder_is_in_or_the_only_one() {
    let both = workspace(&[gain(), eq()]);
    let inside = both
        .plugin(None, Path::new("/w/examples/eq/src"))
        .expect("the EQ's folder");
    assert_eq!(inside.package, "hapto-example-eq");
    let refused = both
        .plugin(None, Path::new("/w"))
        .expect_err("two to pick from");
    assert!(
        format!("{refused:#}").contains("hapto-example-gain, hapto-example-eq"),
        "{refused}"
    );
    let only = workspace(&[gain(), package("tool", "/w/tool", &["bin"], None)]);
    let found = only
        .plugin(None, Path::new("/w/tool"))
        .expect("the only one");
    assert_eq!(found.package, "hapto-example-gain");
    let none = workspace(&[package("tool", "/w/tool", &["bin"], None)]);
    assert!(none.plugin(None, Path::new("/w")).is_err());
}

#[test]
fn the_standalone_binary_is_the_one_that_needs_the_standalone_feature() {
    let mut gain = gain();
    gain["targets"] = serde_json::json!([
        { "name": "hapto_example_gain", "kind": ["cdylib", "rlib"] },
        { "name": "tool", "kind": ["bin"] },
        {
            "name": "hapto-example-gain-standalone",
            "kind": ["bin"],
            "required-features": ["standalone"],
        },
        { "name": "page_params", "kind": ["test"] },
    ]);
    let plugin = workspace(&[gain])
        .plugin(None, Path::new("/w"))
        .expect("a plugin");
    assert_eq!(
        plugin.standalone.as_deref(),
        Some("hapto-example-gain-standalone")
    );
    assert!(plugin.has_param_map_test);
}

#[test]
fn the_locked_version_is_the_one_line_after_the_name() {
    let lockfile = "version = 4\n\n[[package]]\nname = \"hapto-host\"\nversion = \"0.1.0\"\n\
        dependencies = [\n \"vst3\",\n]\n\n[[package]]\nname = \"vst3\"\nversion = \"0.3.0\"\n";
    assert_eq!(locked_version(lockfile, "hapto-host"), Some("0.1.0"));
    assert_eq!(locked_version(lockfile, "vst3"), Some("0.3.0"));
    assert_eq!(locked_version(lockfile, "hapto-params"), None, "not locked");
    let twice = format!("{lockfile}\n[[package]]\nname = \"vst3\"\nversion = \"0.4.0\"\n");
    assert_eq!(locked_version(&twice, "vst3"), None, "two versions");
}
