//! A `cargo build`'s artifacts are read from its JSON messages: where it
//! wrote each target's files, and whether it wrote them at all.

use std::path::PathBuf;

use super::Artifact;

#[test]
fn a_binary_s_executable_and_freshness_are_read() {
    let line = r#"{"reason":"compiler-artifact","package_id":"path+file:///w/gain#0.1.0","manifest_path":"/w/gain/Cargo.toml","target":{"kind":["bin"],"crate_types":["bin"],"name":"gain-standalone","src_path":"/w/gain/src/bin/standalone.rs","edition":"2024","doc":true,"doctest":false,"test":true},"profile":{"opt_level":"0","debuginfo":2,"debug_assertions":true,"overflow_checks":true,"test":false},"features":["standalone"],"filenames":["/w/target/x86_64-pc-windows-msvc/debug/gain-standalone.exe","/w/target/x86_64-pc-windows-msvc/debug/gain_standalone.pdb"],"executable":"/w/target/x86_64-pc-windows-msvc/debug/gain-standalone.exe","fresh":true}"#;
    let artifact = Artifact::read(line).expect("an artifact");
    assert_eq!(artifact.target, "gain-standalone");
    assert_eq!(artifact.kinds, ["bin"]);
    assert_eq!(
        artifact.executable,
        Some(PathBuf::from(
            "/w/target/x86_64-pc-windows-msvc/debug/gain-standalone.exe"
        )),
        "where Cargo wrote it, under the target's own folder"
    );
    assert!(artifact.fresh);
}

#[test]
fn a_library_s_files_are_read() {
    let line = r#"{"reason":"compiler-artifact","target":{"kind":["cdylib","rlib"],"name":"gain"},"filenames":["/w/target/release/gain.dll","/w/target/release/gain.dll.lib","/w/target/release/libgain.rlib"],"executable":null,"fresh":false}"#;
    let artifact = Artifact::read(line).expect("an artifact");
    assert_eq!(artifact.kinds, ["cdylib", "rlib"]);
    assert_eq!(
        artifact.files[0],
        PathBuf::from("/w/target/release/gain.dll")
    );
    assert_eq!(artifact.executable, None);
    assert!(!artifact.fresh);
}

#[test]
fn a_message_that_names_no_artifact_is_passed_over() {
    for line in [
        r#"{"reason":"build-finished","success":true}"#,
        r#"{"reason":"build-script-executed","package_id":"x"}"#,
        "not JSON at all",
        "",
    ] {
        assert_eq!(Artifact::read(line), None, "{line}");
    }
}
