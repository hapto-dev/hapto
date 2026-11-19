//! A command's arguments: a port is a port to listen on, and a request for
//! help is seen wherever it stands.

use super::{Args, wants_help};

fn args(list: &[&str]) -> Args<std::vec::IntoIter<String>> {
    let list: Vec<String> = list.iter().map(|a| (*a).to_owned()).collect();
    Args::new(list, "usage")
}

#[test]
fn a_port_is_one_to_listen_on() {
    assert_eq!(args(&["5200"]).port("--port"), Ok(5200));
    for port in ["0", "70000", "high", "-1"] {
        assert!(args(&[port]).port("--port").is_err(), "{port}");
    }
    assert!(args(&[]).port("--port").is_err(), "none follows");
}

#[test]
fn a_request_for_help_is_seen_wherever_it_stands() {
    let list = |list: &[&str]| list.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
    assert!(wants_help(&list(&["--help"])));
    assert!(wants_help(&list(&["-p", "gain", "-h"])));
    assert!(!wants_help(&list(&["-p", "gain"])));
}
