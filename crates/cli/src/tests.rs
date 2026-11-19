//! The command line: clap accepts the declarations, and each command's help
//! and errors come from them.

use clap::{CommandFactory, Parser};

use super::{Cli, Command};

#[test]
fn the_declarations_are_sound() {
    Cli::command().debug_assert();
}

#[test]
fn each_command_is_read_by_its_name_and_says_its_help() {
    let parse = |args: &[&str]| Cli::try_parse_from(args);
    assert!(matches!(
        parse(&["cargo hapto", "dev", "--host"]).map(|cli| cli.command),
        Ok(Command::Dev(_))
    ));
    let help = parse(&["cargo hapto", "serve", "--help"]).expect_err("help, not a run");
    assert_eq!(help.kind(), clap::error::ErrorKind::DisplayHelp);
    assert!(help.to_string().contains("--port"), "{help}");
    let unknown = parse(&["cargo hapto", "bundle"]).expect_err("no such command");
    assert!(
        unknown.to_string().contains("build"),
        "names what there is: {unknown}"
    );
}
