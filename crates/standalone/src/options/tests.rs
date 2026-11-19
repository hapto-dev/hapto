//! The app reads its signal, its dev session and the dev host from its
//! command line, and refuses what it does not take.

use std::path::PathBuf;

use super::Options;
use crate::signal::Signal;

fn parse(args: &[&str]) -> Result<Options, String> {
    Options::parse(args.iter().map(|a| (*a).to_owned()))
}

#[test]
fn with_no_arguments_the_inputs_hear_silence_with_no_session() {
    let options = parse(&[]).expect("valid");
    assert_eq!(
        options,
        Options {
            signal: Signal::Silence,
            dev: None,
            host: false,
        }
    );
}

#[test]
fn the_signal_the_session_and_the_host_are_read() {
    let options =
        parse(&["--signal", "noise", "--dev", "target/hapto/dev", "--host"]).expect("valid");
    assert_eq!(options.signal, Signal::Noise);
    assert_eq!(options.dev, Some(PathBuf::from("target/hapto/dev")));
    assert!(options.host);
    assert_eq!(
        parse(&["--signal", "sine"]).expect("valid").signal,
        Signal::Sine
    );
}

#[test]
fn what_it_does_not_take_is_refused_with_the_usage() {
    for args in [
        &["--signal"][..],
        &["--signal", "pink"],
        &["--dev"],
        &["--loud"],
    ] {
        let refused = parse(args).expect_err("refused");
        assert!(refused.contains("usage:"), "{args:?}: {refused}");
    }
}
