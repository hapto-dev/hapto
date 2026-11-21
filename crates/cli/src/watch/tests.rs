//! A rebuild is due when a watched file is written, added or removed, and
//! only then.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use super::Watch;

fn member(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("cargo-hapto-tests")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src/bin")).expect("made");
    std::fs::create_dir_all(dir.join("page/src")).expect("made");
    std::fs::write(dir.join("Cargo.toml"), "[package]").expect("written");
    std::fs::write(dir.join("src/lib.rs"), "").expect("written");
    dir
}

/// Sets a file's written time a second later than any before, since a file
/// system may keep times coarser than a test runs.
fn touch(file: &std::path::Path, seconds: u64) {
    let time = SystemTime::now() + Duration::from_secs(seconds);
    let handle = std::fs::File::options()
        .write(true)
        .open(file)
        .expect("opened");
    handle.set_modified(time).expect("dated");
}

#[test]
fn a_written_added_or_removed_source_changes_the_fingerprint() {
    let dir = member("sources");
    let watch = Watch::new(&dir, [dir.as_path()]);
    let before = watch.fingerprint();
    assert_eq!(watch.fingerprint(), before, "nothing changed");

    touch(&dir.join("src/lib.rs"), 10);
    let written = watch.fingerprint();
    assert_ne!(written, before);

    std::fs::write(dir.join("src/bin/standalone.rs"), "").expect("written");
    let added = watch.fingerprint();
    assert_ne!(added, written);

    std::fs::remove_file(dir.join("src/bin/standalone.rs")).expect("removed");
    assert_ne!(watch.fingerprint(), added);

    touch(&dir.join("Cargo.toml"), 20);
    assert_ne!(watch.fingerprint(), added);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_editor_s_temporary_files_are_not_watched_but_an_included_one_is() {
    let dir = member("editors");
    let watch = Watch::new(&dir, [dir.as_path()]);
    let before = watch.fingerprint();
    let editors = [
        ".lib.rs.swp",
        "lib.rs~",
        "lib.rs___jb_tmp___",
        "save.tmp",
        "#lib.rs#",
    ];
    for temporary in editors {
        std::fs::write(dir.join("src").join(temporary), "").expect("written");
    }
    assert_eq!(watch.fingerprint(), before, "an editor's own files");
    std::fs::write(dir.join("src/page.html"), "").expect("written");
    assert_ne!(watch.fingerprint(), before, "a file include_str! reads");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_file_swapped_for_one_as_old_changes_the_fingerprint() {
    let dir = member("swapped");
    touch(&dir.join("src/lib.rs"), 10);
    std::fs::write(dir.join("src/a.rs"), "").expect("written");
    touch(&dir.join("src/a.rs"), 2);
    let watch = Watch::new(&dir, [dir.as_path()]);
    let before = watch.fingerprint();
    let time = std::fs::metadata(dir.join("src/a.rs"))
        .and_then(|m| m.modified())
        .expect("dated");
    std::fs::rename(dir.join("src/a.rs"), dir.join("src/b.rs")).expect("moved");
    let moved = std::fs::File::options()
        .write(true)
        .open(dir.join("src/b.rs"))
        .expect("opened");
    moved.set_modified(time).expect("dated");
    assert_ne!(watch.fingerprint(), before, "as many files, none newer");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_page_and_target_are_not_watched() {
    let dir = member("page");
    std::fs::create_dir_all(dir.join("target/debug")).expect("made");
    let watch = Watch::new(&dir, [dir.as_path()]);
    let before = watch.fingerprint();
    std::fs::write(dir.join("page/src/main.tsx"), "").expect("written");
    std::fs::write(dir.join("target/debug/out.rs"), "").expect("written");
    assert_eq!(watch.fingerprint(), before);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_workspace_s_own_files_are_watched_too() {
    let root = member("workspace");
    let plugin = root.join("plugin");
    std::fs::create_dir_all(plugin.join("src")).expect("made");
    std::fs::write(plugin.join("Cargo.toml"), "[package]").expect("written");
    let watch = Watch::new(&root, [plugin.as_path()]);
    let before = watch.fingerprint();
    std::fs::write(root.join("Cargo.lock"), "").expect("written");
    assert_eq!(
        watch.fingerprint(),
        before,
        "the lockfile, which a build writes"
    );
    std::fs::create_dir_all(root.join(".cargo")).expect("made");
    std::fs::write(root.join(".cargo/config.toml"), "").expect("written");
    let configured = watch.fingerprint();
    assert_ne!(configured, before, "Cargo's configuration");
    touch(&root.join("Cargo.toml"), 10);
    assert_ne!(watch.fingerprint(), configured, "the workspace's manifest");
    let _ = std::fs::remove_dir_all(root);
}
