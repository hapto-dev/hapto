//! A format's layout places the module in the bundle and finds the install
//! folders, and a format with no layout is refused by name.

use std::ffi::OsString;
use std::path::Path;

use super::{Arch, Folder, Format, Layout, PE};

#[test]
fn the_bundle_holds_the_module_under_its_architecture() {
    let root = Path::new("target/bundle");
    for (arch, folder) in [(Arch::X86_64, "x86_64-win"), (Arch::Aarch64, "arm64-win")] {
        let (bundle, module) = PE.paths(root, "Gain", arch);
        assert_eq!(bundle, Path::new("target/bundle/Gain.vst3"));
        let contents = Path::new("target/bundle/Gain.vst3/Contents");
        assert_eq!(module, contents.join(folder).join("Gain.vst3"), "{arch:?}");
    }
}

#[test]
fn only_pe_modules_have_a_bundle_layout_yet() {
    let pe = Layout::of(Format::Pe).expect("PE has a layout");
    assert!(std::ptr::eq(pe, &PE));
    for format in [Format::Elf, Format::MachO] {
        let refused = Layout::of(format).expect_err("no layout").to_string();
        assert!(refused.contains(&format.to_string()), "{refused}");
    }
}

#[test]
fn each_install_folder_is_under_its_variable() {
    let var = |name: &str| Some(OsString::from(format!("/{name}")));
    assert_eq!(
        PE.install_folder(Folder::User, var).unwrap(),
        Path::new("/LOCALAPPDATA/Programs/Common/VST3")
    );
    assert_eq!(
        PE.install_folder(Folder::System, var).unwrap(),
        Path::new("/CommonProgramFiles/VST3")
    );
    let unset = PE.install_folder(Folder::System, |_| None);
    let unset = unset.expect_err("unset").to_string();
    assert!(unset.contains("CommonProgramFiles"), "{unset}");
}

#[test]
fn each_folder_names_the_other() {
    assert_eq!(Folder::User.other(), Folder::System);
    assert_eq!(Folder::System.other(), Folder::User);
}
