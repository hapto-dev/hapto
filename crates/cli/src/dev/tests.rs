//! `cargo hapto dev` reads its mode, and builds and runs the app's binary
//! with the features and arguments each mode needs.

use std::path::{Path, PathBuf};

use super::{
    App, Build, Copies, DEV_DOES, Ended, Mode, Next, Options, PageSource, RELAUNCH, Seen,
    after_build, app_command, page_source,
};
use crate::bundle::{Profile, windows_only};
use crate::page::{DEV_PORT, PAGE_DEV_URL};
use crate::workspace::Plugin;

fn parse(args: &[&str]) -> Result<Options, clap::Error> {
    crate::parse_as::<super::Line>(args).map(Options::from)
}

fn gain(standalone: Option<&str>) -> Plugin {
    Plugin {
        package: "hapto-example-gain".into(),
        name: "Hapto Example Gain".into(),
        lib: "hapto_example_gain".into(),
        dir: PathBuf::from("/w/examples/gain"),
        standalone: standalone.map(str::to_owned),
        has_param_map_test: true,
    }
}

#[test]
fn with_no_arguments_the_plugin_runs_standalone_in_debug_with_its_page() {
    let options = parse(&[]).expect("valid");
    assert_eq!(
        options,
        Options {
            package: None,
            mode: Mode::Standalone,
            signal: None,
            port: DEV_PORT,
            page: true,
            profile: Profile::Debug,
        }
    );
}

#[test]
fn host_is_the_dev_host_and_names_no_host_of_its_own() {
    assert_eq!(parse(&["--host"]).expect("valid").mode, Mode::DevHost);
    let options = parse(&["--host", "-p", "gain", "--release"]).expect("valid");
    assert_eq!(options.mode, Mode::DevHost);
    assert_eq!(options.package.as_deref(), Some("gain"));
    assert_eq!(options.profile, Profile::Release);
    // A real host takes the debug bundle, and its page from `serve`.
    let refused = parse(&["--host", "C:/Hosts/Host.exe"]).expect_err("no host of its own");
    assert!(
        format!("{refused:#}").contains("C:/Hosts/Host.exe"),
        "{refused}"
    );
}

#[test]
fn the_signal_port_and_page_are_read_and_bad_ones_refused() {
    let options = parse(&["--signal", "noise", "--port", "5200", "--no-page"]).expect("valid");
    assert_eq!(options.signal.as_deref(), Some("noise"));
    assert_eq!(options.port, 5200);
    assert!(!options.page);
    assert!(parse(&["--port", "high"]).is_err());
    assert!(parse(&["--signal"]).is_err());
    assert!(parse(&["--install"]).is_err());
}

#[test]
fn the_standalone_app_runs_with_its_dev_session() {
    let options = parse(&["--signal", "silence"]).expect("valid");
    let app = App::of(
        &gain(Some("gain-standalone")),
        &options,
        Path::new("/w/target/hapto/dev/gain"),
    )
    .expect("an app");
    assert_eq!(app.bin, "gain-standalone");
    assert_eq!(app.features, "standalone");
    let path = Path::new("/w/target/hapto/dev/gain").display().to_string();
    assert_eq!(app.args, ["--dev", path.as_str(), "--signal", "silence"]);
}

#[test]
fn the_dev_host_is_the_same_binary_with_its_feature_and_flag() {
    let options = parse(&["--host"]).expect("valid");
    let app = App::of(&gain(Some("gain-standalone")), &options, Path::new("s")).expect("an app");
    assert_eq!(app.features, "standalone,hapto-standalone/dev-host");
    assert_eq!(app.args, ["--dev", "s", "--host"]);
}

#[test]
fn a_crate_with_no_standalone_binary_is_told_how_to_add_one() {
    let refused =
        App::of(&gain(None), &parse(&[]).expect("valid"), Path::new("s")).expect_err("none");
    assert!(
        format!("{refused:#}").contains("required-features"),
        "{refused}"
    );
}

#[test]
fn each_run_is_a_fresh_numbered_copy_of_the_build() {
    let dir = std::env::temp_dir()
        .join("cargo-hapto-tests")
        .join(format!("copies-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("made");
    let built = dir.join(format!("app{}", std::env::consts::EXE_SUFFIX));
    std::fs::write(&built, b"first").expect("written");
    let mut copies = Copies::new(dir.join("runs"));
    let first = copies.next(&built).expect("copied");
    std::fs::write(&built, b"second").expect("rebuilt");
    let second = copies.next(&built).expect("copied");
    assert_ne!(first, second);
    assert_eq!(std::fs::read(&first).expect("read"), b"first");
    assert_eq!(std::fs::read(&second).expect("read"), b"second");
    let name = second
        .file_name()
        .expect("a name")
        .to_string_lossy()
        .into_owned();
    assert!(name.starts_with("app-2"), "{name}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn copies_an_earlier_session_left_are_cleared_and_one_still_there_is_passed() {
    let dir = std::env::temp_dir()
        .join("cargo-hapto-tests")
        .join(format!("stale-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("runs")).expect("made");
    let exe = |name: &str| format!("{name}{}", std::env::consts::EXE_SUFFIX);
    let built = dir.join(exe("app"));
    std::fs::write(&built, b"built").expect("written");
    for left in ["app-1", "app-7"] {
        std::fs::write(dir.join("runs").join(exe(left)), b"old").expect("written");
    }
    let mut copies = Copies::new(dir.join("runs"));
    assert!(!dir.join("runs").join(exe("app-7")).exists(), "cleared");
    // A copy that could not be cleared, as one still running is not.
    std::fs::write(dir.join("runs").join(exe("app-1")), b"running").expect("written");
    let first = copies.next(&built).expect("copied");
    assert_eq!(
        first,
        dir.join("runs").join(exe("app-2")),
        "past the one there"
    );
    std::fs::remove_file(&built).expect("removed");
    let failed = copies.next(&built).expect_err("no build to copy");
    assert!(
        format!("{failed:#}").contains("app-3"),
        "names the copy: {failed}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn only_a_clean_close_ends_the_session_and_a_crash_waits_for_a_build() {
    assert_eq!(Ended::of(Some(RELAUNCH)), Ended::Relaunch);
    assert_eq!(Ended::of(Some(0)), Ended::Closed);
    assert_eq!(Ended::of(Some(101)), Ended::Failed(Some(101)), "a panic");
    assert_eq!(Ended::of(Some(3)), Ended::Failed(Some(3)), "a C abort");
    assert_eq!(Ended::of(None), Ended::Failed(None), "ended with no code");
}

#[test]
fn a_debug_app_takes_its_page_from_the_server_and_a_release_one_built_in() {
    assert_eq!(page_source(true, true, Profile::Debug), PageSource::Server);
    assert_eq!(
        page_source(true, true, Profile::Release),
        PageSource::Built,
        "a release build never reads the server's address"
    );
    assert_eq!(page_source(true, false, Profile::Release), PageSource::None);
    assert_eq!(page_source(false, true, Profile::Debug), PageSource::None);
}

/// A file of this checkout's crates, by its path below `crates/`.
fn source(path: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(path);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn dev_refuses_any_system_but_windows_before_it_starts_anything() {
    assert!(windows_only("windows", DEV_DOES).is_ok());
    for os in ["macos", "linux"] {
        let refused = windows_only(os, DEV_DOES).expect_err(os).to_string();
        assert!(refused.contains("standalone app"), "{refused}");
        assert!(refused.contains(os), "{refused}");
    }
    let run = source("cli/src/dev.rs");
    let first = run
        .split("pub(crate) fn run(options: &Options) -> Result<()> {")
        .nth(1)
        .and_then(|body| body.lines().find(|line| !line.trim().is_empty()))
        .unwrap_or_default();
    assert_eq!(
        first.trim(),
        "windows_only(std::env::consts::OS, DEV_DOES)?;",
        "the first thing run does"
    );
}

#[test]
fn the_relaunch_code_and_the_page_variable_are_the_ones_the_app_uses() {
    // This command links no Hapto crate, so it keeps copies; these are the
    // lines the app's crates hold them in.
    let relaunch = format!("pub(crate) const RELAUNCH: u8 = {RELAUNCH};");
    assert!(
        source("standalone/src/session.rs").contains(&relaunch),
        "{relaunch}"
    );
    let read = format!("std::env::var(\"{PAGE_DEV_URL}\")");
    assert!(source("webview/src/assets.rs").contains(&read), "{read}");
}

#[test]
fn an_app_closed_while_it_rebuilt_ends_the_session() {
    use Build::{Failed, New, Unchanged};
    use Ended::{Closed, Relaunch};
    let crashed = Seen::Ended(Ended::Failed(Some(1)));
    let cases = [
        // How the app was when the build ended, what the build did, and next.
        (Seen::Ended(Closed), New, Next::End),
        (Seen::Ended(Closed), Failed, Next::End),
        (Seen::Ended(Relaunch), New, Next::Start),
        (Seen::Ended(Relaunch), Failed, Next::Start),
        (Seen::Running, New, Next::Start),
        (Seen::Running, Unchanged, Next::Keep),
        (Seen::Running, Failed, Next::Keep),
        (crashed, New, Next::Start),
        (crashed, Unchanged, Next::Start),
        (crashed, Failed, Next::Wait),
        (Seen::Stopped, Unchanged, Next::Start),
        (Seen::Stopped, Failed, Next::Wait),
    ];
    for (seen, build, next) in cases {
        assert_eq!(after_build(seen, build), next, "{seen:?}, {build:?}");
    }
}

#[test]
fn the_app_reads_its_page_from_this_server_or_from_none() {
    use std::ffi::OsString;
    let envs = |url| {
        app_command(Path::new("app"), &[], url)
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(ToOwned::to_owned)))
            .collect::<Vec<_>>()
    };
    let url = "http://127.0.0.1:5173";
    assert_eq!(
        envs(Some(url)),
        [(OsString::from(PAGE_DEV_URL), Some(OsString::from(url)))]
    );
    assert_eq!(
        envs(None),
        [(OsString::from(PAGE_DEV_URL), None)],
        "removed, so one the environment set is not read"
    );
}
