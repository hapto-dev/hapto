//! A session keeps the saved block between runs, writes it only when it
//! changed, and keeps the dev host's format.

use std::path::PathBuf;

use super::Session;

/// A fresh folder of the test's own under the system's temporary folder.
fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("hapto-standalone-tests")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn a_kept_block_is_there_at_the_next_run() {
    let dir = folder("kept");
    let mut first = Session::in_dir(dir.clone()).expect("made");
    assert_eq!(first.saved_block(), Ok(None), "a new session holds none");
    first.keep(b"{\"one\":1}".to_vec()).expect("written");
    let mut second = Session::in_dir(dir.clone()).expect("opened");
    assert_eq!(
        second.saved_block().expect("read").as_deref(),
        Some(&b"{\"one\":1}"[..])
    );
    assert!(!dir.join("saved.json.new").exists(), "moved over the old");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_unchanged_block_is_not_written_again() {
    let dir = folder("unchanged");
    let mut session = Session::in_dir(dir.clone()).expect("made");
    session.keep(b"a".to_vec()).expect("written");
    std::fs::write(dir.join("saved.json"), b"changed elsewhere").expect("written");
    session.keep(b"a".to_vec()).expect("nothing to write");
    assert_eq!(
        std::fs::read(dir.join("saved.json")).expect("read"),
        b"changed elsewhere"
    );
    session.keep(b"b".to_vec()).expect("written");
    assert_eq!(std::fs::read(dir.join("saved.json")).expect("read"), b"b");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_held_session_writes_nothing_over_its_block() {
    let dir = folder("held");
    let mut session = Session::in_dir(dir.clone()).expect("made");
    std::fs::write(dir.join("saved.json"), b"refused").expect("written");
    assert_eq!(session.saved_block(), Ok(Some(b"refused".to_vec())));
    session.hold();
    session
        .keep(b"defaults".to_vec())
        .expect("nothing to write");
    assert_eq!(
        std::fs::read(dir.join("saved.json")).expect("read"),
        b"refused"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_block_that_cannot_be_read_is_held_not_taken_for_none() {
    let dir = folder("unreadable");
    // A folder where the block should be: there, and not readable as one.
    std::fs::create_dir_all(dir.join("saved.json")).expect("made");
    let mut session = Session::in_dir(dir.clone()).expect("opened");
    assert!(session.saved_block().is_err(), "an error, not None");
    session
        .keep(b"defaults".to_vec())
        .expect("nothing to write");
    assert!(dir.join("saved.json").is_dir(), "left as it was");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_dev_session_is_the_folder_it_is_given() {
    let dir = folder("dev");
    let mut session = Session::open("dev.test.amp", Some(&dir)).expect("made");
    session.keep(b"x".to_vec()).expect("written");
    assert!(dir.join("saved.json").is_file());
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(feature = "dev-host")]
#[test]
fn the_dev_hosts_format_is_kept() {
    let dir = folder("format");
    let session = Session::in_dir(dir.clone()).expect("made");
    assert_eq!(session.format(), None);
    session.set_format("cabi").expect("written");
    assert_eq!(
        Session::in_dir(dir.clone())
            .expect("opened")
            .format()
            .as_deref(),
        Some("cabi")
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(feature = "dev-host")]
#[test]
fn the_dev_hosts_audio_settings_are_kept() {
    let dir = folder("audio");
    let session = Session::in_dir(dir.clone()).expect("made");
    assert_eq!(session.audio(), None);
    session.set_audio("96000 256 random").expect("written");
    assert_eq!(
        Session::in_dir(dir.clone())
            .expect("opened")
            .audio()
            .as_deref(),
        Some("96000 256 random")
    );
    let _ = std::fs::remove_dir_all(dir);
}
