//! Cargo can't read a cfg that a build script sets, so a crate that sets one
//! per system must name the same systems in its target tables.
//! [`assert_target_tables_match`] checks that they agree.
//!
//! # Examples
//!
//! The build script passes its list to the crate with
//! `cargo::rustc-env=MY_SYSTEMS=windows,linux`, and a test of the crate reads
//! it with `env!("MY_SYSTEMS")`:
//!
//! ```
//! use hapto_testkit::systems::assert_target_tables_match;
//!
//! let manifest = r#"
//! [target.'cfg(any(target_os = "windows", target_os = "linux"))'.dependencies]
//! wry = "0.57"
//! "#;
//! assert_target_tables_match(manifest, "linux,windows");
//! ```

use std::collections::BTreeSet;

/// Checks that the `target_os` values in the target tables of `manifest` are
/// exactly the systems in `listed`, a comma-separated list such as a build
/// script passes to its crate. The order doesn't matter, and a line that
/// doesn't start a table, such as a comment, is not read. Every `target_os`
/// that a table's cfg mentions counts as named, even inside a `not(..)`.
///
/// # Panics
///
/// Panics when the two sets differ, and when `listed` names no system, so a
/// check that reads nothing fails.
#[track_caller]
pub fn assert_target_tables_match(manifest: &str, listed: &str) {
    let listed: BTreeSet<&str> = listed
        .split(',')
        .filter(|system| !system.is_empty())
        .collect();
    assert!(!listed.is_empty(), "the build script lists no system");
    let named: BTreeSet<&str> = manifest
        .lines()
        .filter(|line| line.starts_with("[target."))
        // Each piece after `target_os = "` starts with a system's name.
        .flat_map(|line| line.split("target_os = \"").skip(1))
        .filter_map(|rest| rest.split_once('"').map(|(system, _)| system))
        .collect();
    assert_eq!(
        named, listed,
        "the systems that the manifest's target tables name, and the ones that the \
         build script lists"
    );
}

#[cfg(test)]
mod tests {
    use super::assert_target_tables_match;

    const TABLES: &str = r#"
# [target.'cfg(target_os = "macos")'.dependencies] is commented out.
[target.'cfg(target_os = "windows")'.dependencies]
wry = "0.57"

[target.'cfg(target_os = "linux")'.dependencies]
gtk = "0.18"
"#;

    #[test]
    fn tables_that_name_the_listed_systems_in_any_order_agree() {
        assert_target_tables_match(TABLES, "linux,windows");
        assert_target_tables_match(TABLES, "windows,linux");
    }

    #[test]
    #[should_panic(expected = "the systems that the manifest's target tables name")]
    fn a_table_for_a_system_that_is_not_listed_fails() {
        assert_target_tables_match(TABLES, "windows");
    }

    #[test]
    #[should_panic(expected = "the systems that the manifest's target tables name")]
    fn a_listed_system_with_no_table_fails() {
        assert_target_tables_match(TABLES, "windows,linux,macos");
    }

    #[test]
    #[should_panic(expected = "lists no system")]
    fn an_empty_list_fails() {
        assert_target_tables_match(TABLES, "");
    }
}
