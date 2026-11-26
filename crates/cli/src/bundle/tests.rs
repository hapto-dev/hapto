//! `cargo hapto build` reads its arguments, lays a bundle out where a host
//! looks, and words a failed copy so the user knows what to do.

use std::io;
use std::path::{Path, PathBuf};

use super::format::Arch;
use super::{Folder, Format, Layout, Options, Profile, copy_error, file_name, module_of};
use crate::tool::Artifact;
use crate::workspace::Plugin;

fn parse(args: &[&str]) -> Result<Options, clap::Error> {
    crate::parse_as::<super::Line>(args).map(Options::from)
}

#[test]
fn the_crate_and_flags_are_read() {
    let options = parse(&["-p", "gain", "--no-page", "--install"]).expect("valid");
    assert_eq!(
        options,
        Options {
            package: Some("gain".into()),
            page: false,
            profile: Profile::Release,
            install: Some(Folder::User),
        }
    );
    let options = parse(&["--install-system", "--package", "gain"]).expect("valid");
    assert!(options.page);
    assert_eq!(options.install, Some(Folder::System));
    let options = parse(&["--debug"]).expect("valid");
    assert_eq!((options.package, options.profile), (None, Profile::Debug));
}

#[test]
fn each_profile_builds_with_its_own_arguments() {
    assert_eq!(Profile::Release.cargo_args(), ["--release"]);
    assert!(Profile::Debug.cargo_args().is_empty());
}

#[test]
fn bad_arguments_are_refused() {
    assert!(parse(&["-p"]).is_err(), "-p with no name");
    assert!(parse(&["-p", "gain", "--release"]).is_err(), "unknown flag");
    let both = parse(&["-p", "gain", "--install", "--install-system"]);
    assert!(both.is_err(), "both install folders");
    let twice = parse(&["-p", "gain", "--install", "--install"]);
    assert!(twice.is_err(), "a flag given twice");
}

#[test]
fn a_name_no_file_can_hold_still_lays_the_bundle_out_in_its_folder() {
    let root = Path::new("target/bundle");
    let layout = Layout::of(Format::Pe).expect("PE has a layout");
    for name in ["../escape", "C:\\elsewhere", "A/B Gain", "EQ: Pro"] {
        let file = file_name(name).expect("a usable name");
        let (bundle, module) = layout.paths(root, &file, Arch::X86_64);
        assert_eq!(bundle.parent(), Some(root), "{name}");
        assert!(module.starts_with(&bundle), "{name}");
    }
}

#[test]
fn a_name_is_written_as_a_file_name_or_refused() {
    for (name, file) in [
        ("Hapto Example Gain", "Hapto Example Gain"),
        ("A/B Gain", "A-B Gain"),
        ("EQ: Pro", "EQ- Pro"),
        ("C:\\elsewhere", "C--elsewhere"),
        ("Gain...", "Gain"),
        ("Bell\u{7}", "Bell-"),
    ] {
        assert_eq!(file_name(name).unwrap(), file, "{name:?}");
    }
    for name in ["", "   ", "...", "CON", "nul", "Com1"] {
        assert!(file_name(name).is_err(), "{name:?}");
    }
    // A device's name with anything after a dot is the device still, and
    // Windows keeps a 0 and the superscript digits too.
    for name in ["Aux.Send", "nul.txt", "CON .wav", "COM0", "LPT\u{b9}"] {
        assert!(file_name(name).is_err(), "{name:?}");
    }
    for name in ["Auxiliary", "Console", "Con Fuoco", "COM10"] {
        assert!(file_name(name).is_ok(), "{name:?}");
    }
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
fn the_module_is_the_one_cargo_wrote_for_the_cdylib() {
    let plugin = Plugin {
        package: "gain".into(),
        name: "Gain".into(),
        lib: "gain".into(),
        dir: PathBuf::from("/w/gain"),
        standalone: None,
        has_param_map_test: false,
    };
    let dll = format!(
        "{}gain{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
    let under = PathBuf::from("/w/target/x86_64-pc-windows-msvc/release");
    let artifact = |target: &str, kinds: &[&str], files: Vec<PathBuf>| Artifact {
        target: target.into(),
        kinds: kinds.iter().map(|kind| (*kind).to_owned()).collect(),
        files,
        executable: None,
        fresh: false,
    };
    let built = [
        artifact(
            "hapto_host",
            &["lib"],
            vec![under.join("libhapto_host.rlib")],
        ),
        artifact(
            "gain",
            &["cdylib", "rlib"],
            vec![under.join(format!("{dll}.lib")), under.join(&dll)],
        ),
    ];
    assert_eq!(module_of(&built, &plugin).unwrap(), under.join(&dll));
    assert!(module_of(&built[..1], &plugin).is_err(), "no cdylib built");
}

#[test]
fn build_looks_up_the_layout_before_it_starts_anything() {
    let first = include_str!("../bundle.rs")
        .split(") -> Result<Bundle> {")
        .nth(1)
        .and_then(|body| {
            body.lines()
                .map(str::trim)
                .find(|line| !line.is_empty() && !line.starts_with("//"))
        })
        .unwrap_or_default();
    assert_eq!(
        first, "Layout::of(Format::of_system(std::env::consts::OS))?;",
        "the first thing build does"
    );
}
