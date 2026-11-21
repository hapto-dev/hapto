//! `cargo hapto serve` reads the crate and the port, and refuses what it
//! does not take.

use super::Options;
use crate::page::DEV_PORT;

fn parse(args: &[&str]) -> Result<Options, clap::Error> {
    crate::parse_as::<Options>(args)
}

#[test]
fn with_no_arguments_it_serves_the_found_crate_on_the_dev_port() {
    assert_eq!(
        parse(&[]).expect("valid"),
        Options {
            package: None,
            port: DEV_PORT,
        }
    );
}

#[test]
fn the_crate_and_port_are_read_and_bad_ones_refused() {
    let options = parse(&["-p", "gain", "--port", "5200"]).expect("valid");
    assert_eq!(options.package.as_deref(), Some("gain"));
    assert_eq!(options.port, 5200);
    assert!(parse(&["--port", "high"]).is_err());
    assert!(parse(&["-p"]).is_err());
    assert!(parse(&["--host"]).is_err(), "it starts no host");
}
