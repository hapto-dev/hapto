//! `cargo xtask bundle` reads its arguments and `cargo metadata`, lays a
//! bundle out where a host looks, checks a page's types before building it,
//! runs `npm ci` only when needed, and words a failed copy so the user knows
//! what to do.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use xtask::bundle::{
    Folder, Options, Plugin, Profile, arch_folder, copy_error, layout, needs_npm_ci, page_scripts,
    plugin,
};

fn parse(args: &[&str]) -> Result<Options, String> {
    Options::parse(args.iter().map(|a| (*a).to_owned()))
}

#[test]
fn the_crate_and_flags_are_read() {
    let options = parse(&["-p", "gain", "--no-page", "--install"]).expect("valid");
    assert_eq!(
        options,
        Options {
            package: "gain".into(),
            page: false,
            profile: Profile::Release,
            install: Some(Folder::User),
        }
    );
    let options = parse(&["--install-system", "--package", "gain"]).expect("valid");
    assert!(options.page);
    assert_eq!(options.install, Some(Folder::System));
    let options = parse(&["-p", "gain", "--debug"]).expect("valid");
    assert_eq!(options.profile, Profile::Debug);
}

#[test]
fn each_profile_builds_and_reads_its_own_folder() {
    assert_eq!(Profile::Release.cargo_args(), ["--release"]);
    assert_eq!(Profile::Release.dir(), "release");
    assert!(Profile::Debug.cargo_args().is_empty());
    assert_eq!(Profile::Debug.dir(), "debug");
}

#[test]
fn bad_arguments_are_refused() {
    assert!(parse(&[]).is_err(), "no crate");
    assert!(parse(&["-p"]).is_err(), "-p with no name");
    assert!(parse(&["-p", "gain", "--release"]).is_err(), "unknown flag");
    let both = parse(&["-p", "gain", "--install", "--install-system"]);
    assert!(both.is_err(), "both install folders");
    assert!(parse(&["-p", "gain", "--install", "--install"]).is_ok());
}

fn metadata(packages: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "target_directory": "/w/target", "packages": packages })
}

fn package(kinds: &[&str], seam: Option<&str>) -> serde_json::Value {
    let mut package = serde_json::json!({
        "name": "seam-example-gain",
        "manifest_path": "/w/examples/gain/Cargo.toml",
        "targets": [
            { "name": "guide", "kind": ["test"] },
            { "name": "seam_example_gain", "kind": kinds },
        ],
        "metadata": null,
    });
    if let Some(name) = seam {
        package["metadata"] = serde_json::json!({ "seam": { "name": name } });
    }
    package
}

#[test]
fn a_plugin_crate_is_found_in_the_metadata() {
    let data = metadata(&serde_json::json!([package(
        &["cdylib", "rlib"],
        Some("Seam Example Gain")
    )]));
    let (found, target) = plugin(&data, "seam-example-gain").expect("a plugin");
    assert_eq!(
        found,
        Plugin {
            name: "Seam Example Gain".into(),
            lib: "seam_example_gain".into(),
            dir: PathBuf::from("/w/examples/gain"),
        }
    );
    assert_eq!(target, Path::new("/w/target"));
}

#[test]
fn a_crate_that_is_no_plugin_is_refused() {
    let gain = package(&["cdylib", "rlib"], Some("Seam Example Gain"));
    let data = metadata(&serde_json::json!([gain]));
    assert!(
        plugin(&data, "seam-example-eq").is_err(),
        "not in the workspace"
    );
    let no_cdylib = metadata(&serde_json::json!([package(&["rlib"], Some("Gain"))]));
    assert!(
        plugin(&no_cdylib, "seam-example-gain").is_err(),
        "no module"
    );
    let no_name = metadata(&serde_json::json!([package(&["cdylib"], None)]));
    assert!(plugin(&no_name, "seam-example-gain").is_err(), "no name");
    assert!(plugin(&serde_json::json!({}), "seam-example-gain").is_err());
}

#[test]
fn the_bundle_holds_the_module_under_its_architecture() {
    let (bundle, module) = layout(Path::new("target/bundle"), "Gain", "x86_64-win");
    assert_eq!(bundle, Path::new("target/bundle/Gain.vst3"));
    assert_eq!(
        module,
        Path::new("target/bundle/Gain.vst3/Contents/x86_64-win/Gain.vst3")
    );
}

#[test]
fn only_windows_has_a_bundle_layout_yet() {
    assert_eq!(arch_folder("windows", "x86_64"), Ok("x86_64-win"));
    assert_eq!(arch_folder("windows", "aarch64"), Ok("arm64-win"));
    assert!(arch_folder("windows", "x86").is_err());
    assert!(arch_folder("macos", "aarch64").is_err());
    assert!(arch_folder("linux", "x86_64").is_err());
}

#[test]
fn npm_ci_runs_only_when_the_packages_are_missing_or_stale() {
    let then = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let later = then + Duration::from_secs(1);
    assert!(needs_npm_ci(None, Some(then)), "never installed");
    assert!(needs_npm_ci(None, None));
    assert!(
        needs_npm_ci(Some(then), Some(later)),
        "lockfile changed since"
    );
    assert!(!needs_npm_ci(Some(later), Some(then)), "installed since");
    assert!(!needs_npm_ci(Some(then), Some(then)));
    assert!(
        !needs_npm_ci(Some(then), None),
        "npm ci reports the lockfile"
    );
}

#[test]
fn a_held_module_says_to_quit_the_host() {
    let held = io::Error::from_raw_os_error(32);
    for folder in [Folder::User, Folder::System] {
        let message = copy_error(&held, Path::new("Gain.vst3"), folder);
        assert!(message.contains("quit the host first"), "{message}");
    }
}

#[test]
fn a_refused_system_copy_says_it_needs_an_administrator() {
    let refused = io::Error::from(io::ErrorKind::PermissionDenied);
    let message = copy_error(&refused, Path::new("VST3"), Folder::System);
    assert!(message.contains("administrator"), "{message}");
    let message = copy_error(&refused, Path::new("VST3"), Folder::User);
    assert!(!message.contains("administrator"), "{message}");
}

#[test]
fn each_folder_names_the_other() {
    assert_eq!(Folder::User.other(), Folder::System);
    assert_eq!(Folder::System.other(), Folder::User);
}

#[test]
fn a_page_is_type_checked_before_it_is_built() {
    let checked = serde_json::json!({
        "scripts": { "dev": "vite", "build": "vite build", "typecheck": "tsc --noEmit" }
    });
    assert_eq!(page_scripts(&checked), ["typecheck", "build"]);
    let unchecked = serde_json::json!({ "scripts": { "build": "vite build" } });
    assert_eq!(page_scripts(&unchecked), ["build"]);
    assert_eq!(page_scripts(&serde_json::json!({})), ["build"]);
}
