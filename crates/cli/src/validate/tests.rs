//! `cargo hapto validate` reads its arguments, and runs pluginval on the bundle
//! as CI does, keeping its logs where asked.

use std::ffi::OsStr;
use std::path::Path;

use super::{Options, pluginval_command, run_pluginval};

fn parse(args: &[&str]) -> Result<Options, clap::Error> {
    crate::parse_as::<Options>(args)
}

#[test]
fn the_crate_pluginval_and_the_logs_are_read() {
    let options = parse(&["-p", "gain", "--no-page", "--pluginval", "C:/pv.exe"]).expect("valid");
    assert_eq!(options.package.as_deref(), Some("gain"));
    assert!(!options.page);
    assert_eq!(options.pluginval, Some("C:/pv.exe".into()));
    assert_eq!(options.logs, None);
    let logged = parse(&["--logs", "pluginval-logs"]).expect("valid");
    assert_eq!(logged.logs, Some("pluginval-logs".into()));
    assert!(parse(&["--pluginval"]).is_err(), "no path");
    assert!(parse(&["--logs"]).is_err(), "no folder");
    assert!(
        parse(&["--strictness-level", "10"]).is_err(),
        "not this command's"
    );
}

#[test]
fn pluginval_runs_on_the_bundle_at_strictness_5_keeping_its_logs_when_asked() {
    let args = |logs: Option<&str>| -> String {
        let bundle = Path::new("target/bundle/Gain.vst3");
        let command = pluginval_command(OsStr::new("pluginval"), bundle, logs.map(OsStr::new));
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        args.join(" ")
    };
    let common = "--strictness-level 5 --validate-in-process --skip-gui-tests";
    assert_eq!(args(None), format!("{common} target/bundle/Gain.vst3"));
    assert_eq!(
        args(Some("logs")),
        format!("{common} --output-dir logs target/bundle/Gain.vst3")
    );
}

#[test]
fn where_to_find_pluginval_is_said_only_when_it_is_not_found() {
    let missing = run_pluginval(&mut std::process::Command::new(
        "no-such-pluginval-for-this-test",
    ))
    .expect_err("not found");
    assert!(
        format!("{missing:#}").contains("github.com/Tracktion/pluginval"),
        "{missing}"
    );
    // A program that runs and fails, as pluginval does for a bundle that
    // does not pass.
    let failed = run_pluginval(
        crate::tool::cargo()
            .arg("--no-such-flag")
            .stderr(std::process::Stdio::null()),
    )
    .expect_err("it fails");
    assert!(format!("{failed:#}").contains("failed"), "{failed}");
    assert!(
        !format!("{failed:#}").contains("github.com"),
        "it was found: {failed}"
    );
}
