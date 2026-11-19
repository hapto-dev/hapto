//! `cargo hapto build` reads its arguments, lays a bundle out where a host
//! looks, and words a failed copy so the user knows what to do.

use std::io;
use std::path::{Path, PathBuf};

use super::{
    Folder, Options, Profile, arch_folder, copy_error, file_name, layout, module_of, windows_only,
};
use crate::tool::Artifact;
use crate::workspace::Plugin;

fn parse(args: &[&str]) -> Result<Options, String> {
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
fn the_bundle_holds_the_module_under_its_architecture() {
    let (bundle, module) =
        layout(Path::new("target/bundle"), "Gain", "x86_64-win").expect("a name");
    assert_eq!(bundle, Path::new("target/bundle/Gain.vst3"));
    assert_eq!(
        module,
        Path::new("target/bundle/Gain.vst3/Contents/x86_64-win/Gain.vst3")
    );
}

#[test]
fn a_name_no_file_can_hold_still_lays_the_bundle_out_in_its_folder() {
    let root = Path::new("target/bundle");
    for name in ["../escape", "C:\\elsewhere", "A/B Gain", "EQ: Pro"] {
        let (bundle, module) = layout(root, name, "x86_64-win").expect("a usable name");
        assert_eq!(bundle.parent(), Some(root), "{name}");
        assert!(module.starts_with(&bundle), "{name}");
    }
}

#[test]
fn a_name_is_written_as_a_file_name_or_refused() {
    assert_eq!(
        file_name("Hapto Example Gain").as_deref(),
        Ok("Hapto Example Gain")
    );
    assert_eq!(file_name("A/B Gain").as_deref(), Ok("A-B Gain"));
    assert_eq!(file_name("EQ: Pro").as_deref(), Ok("EQ- Pro"));
    assert_eq!(file_name("C:\\elsewhere").as_deref(), Ok("C--elsewhere"));
    assert_eq!(file_name("Gain...").as_deref(), Ok("Gain"));
    assert_eq!(file_name("Bell\u{7}").as_deref(), Ok("Bell-"));
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
fn only_windows_has_a_bundle_layout_yet() {
    assert_eq!(windows_only("windows"), Ok(()));
    assert!(windows_only("macos").is_err());
    assert!(windows_only("linux").is_err());
}

/// The first bytes of a Windows DLL built for `machine`, as its PE header
/// names it.
fn module_for(machine: u16) -> Vec<u8> {
    let mut bytes = vec![0; 0x100];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[0x3c..0x40].copy_from_slice(&0x80_u32.to_le_bytes());
    bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
    bytes[0x84..0x86].copy_from_slice(&machine.to_le_bytes());
    bytes
}

#[test]
fn the_module_goes_under_the_architecture_it_was_built_for() {
    assert_eq!(arch_folder(&module_for(0x8664)), Ok("x86_64-win"));
    assert_eq!(arch_folder(&module_for(0xaa64)), Ok("arm64-win"));
    let x86 = arch_folder(&module_for(0x014c)).expect_err("32-bit x86");
    assert!(x86.contains("0x014c"), "{x86}");
    assert!(arch_folder(b"not a module").is_err());
    assert!(
        arch_folder(&module_for(0x8664)[..0x84]).is_err(),
        "cut short"
    );
}

#[cfg(windows)]
#[test]
fn a_module_built_here_reads_as_this_machine_s() {
    let exe = std::env::current_exe().expect("this test's own executable");
    let head = super::head(&exe).expect("read");
    let expected = match std::env::consts::ARCH {
        "aarch64" => "arm64-win",
        _ => "x86_64-win",
    };
    assert_eq!(arch_folder(&head), Ok(expected));
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
    assert_eq!(module_of(&built, &plugin), Ok(under.join(&dll)));
    assert!(module_of(&built[..1], &plugin).is_err(), "no cdylib built");
}
