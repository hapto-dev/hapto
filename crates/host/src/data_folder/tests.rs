//! Each system's data folder, read from a made-up environment, and a
//! plugin's id written as the name of one folder.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{NoDataFolder, plugin_folder, runtime, user_data};

/// Returns an environment that holds `vars` and nothing else.
fn env(vars: &[(&'static str, &str)]) -> impl Fn(&'static str) -> Option<OsString> {
    move |name| {
        vars.iter()
            .find(|(variable, _)| *variable == name)
            .map(|(_, value)| OsString::from(value))
    }
}

#[test]
fn each_system_s_data_folder_comes_from_its_own_variable() {
    let all = env(&[
        ("LOCALAPPDATA", r"C:\Users\a\AppData\Local"),
        ("HOME", "/home/a"),
        ("XDG_DATA_HOME", "/data/a"),
    ]);
    let home = Path::new("/home/a");
    for (os, folder) in [
        ("windows", PathBuf::from(r"C:\Users\a\AppData\Local")),
        ("macos", home.join("Library").join("Application Support")),
        ("linux", PathBuf::from("/data/a")),
        ("freebsd", PathBuf::from("/data/a")),
    ] {
        assert_eq!(user_data(os, &all), Ok(folder), "{os}");
    }
}

#[test]
fn an_unset_empty_or_relative_xdg_folder_falls_back_to_the_home_s() {
    let shared = Path::new("/home/a").join(".local").join("share");
    for xdg in [None, Some(""), Some("data/a")] {
        let mut vars = vec![("HOME", "/home/a")];
        vars.extend(xdg.map(|value| ("XDG_DATA_HOME", value)));
        assert_eq!(
            user_data("linux", env(&vars)),
            Ok(shared.clone()),
            "{xdg:?}"
        );
    }
}

#[test]
fn the_runtime_folder_is_xdg_s_unless_it_is_unset_empty_or_relative() {
    assert_eq!(
        runtime(env(&[("XDG_RUNTIME_DIR", "/run/user/1000")])),
        Some(PathBuf::from("/run/user/1000"))
    );
    assert_eq!(runtime(env(&[])), None);
    for relative in ["", "run/user/1000"] {
        assert_eq!(
            runtime(env(&[("XDG_RUNTIME_DIR", relative)])),
            None,
            "{relative:?}"
        );
    }
}

#[test]
fn an_unset_or_empty_variable_names_no_folder_and_the_error_names_it() {
    for (os, vars, variable) in [
        ("windows", &[][..], "LOCALAPPDATA"),
        ("windows", &[("LOCALAPPDATA", "")][..], "LOCALAPPDATA"),
        ("macos", &[("HOME", "")][..], "HOME"),
        ("linux", &[("XDG_DATA_HOME", "data/a")][..], "HOME"),
    ] {
        let error = user_data(os, env(vars)).expect_err("no folder");
        assert_eq!(error, NoDataFolder { variable }, "{os}");
        assert_eq!(error.to_string(), format!("{variable} is unset or empty"));
    }
}

#[test]
fn the_plugins_folder_is_its_own_below_the_base() {
    let base = Path::new("base");
    assert_eq!(
        plugin_folder(base, "dev.hapto.example-gain"),
        base.join("dev.hapto.example-gain")
    );
    assert_eq!(plugin_folder(base, "a/b\\c:d e"), base.join("a_b_c_d_e"));
    for dots in ["", ".", ".."] {
        assert_eq!(plugin_folder(base, dots), base.join("hapto-plugin"));
    }
}
