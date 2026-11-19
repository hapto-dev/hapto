//! A page's packages are installed only when needed, from its lockfile when
//! it has one, and its types are checked before it is built.

use std::ffi::OsStr;
use std::path::Path;
use std::time::{Duration, SystemTime};

use super::{DevServer, Packages, claim, linked, packages, param_map_test, scripts};

#[test]
fn packages_are_installed_only_when_missing_or_stale() {
    let then = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let later = then + Duration::from_secs(1);
    assert_eq!(packages(None, Some(then)), Packages::FromLockfile, "never");
    assert_eq!(
        packages(Some(then), Some(later)),
        Packages::FromLockfile,
        "the lockfile changed since"
    );
    assert_eq!(packages(Some(later), Some(then)), Packages::Current);
    assert_eq!(packages(Some(then), Some(then)), Packages::Current);
}

#[test]
fn a_page_with_no_lockfile_resolves_its_packages() {
    let then = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    assert_eq!(packages(None, None), Packages::Resolve);
    assert_eq!(packages(Some(then), None), Packages::Resolve);
}

#[test]
fn a_page_is_type_checked_before_it_is_built() {
    let checked = serde_json::json!({
        "scripts": { "dev": "vite", "build": "vite build", "typecheck": "tsc --noEmit" }
    });
    assert_eq!(scripts(&checked), ["typecheck", "build"]);
    let unchecked = serde_json::json!({ "scripts": { "build": "vite build" } });
    assert_eq!(scripts(&unchecked), ["build"]);
    assert_eq!(scripts(&serde_json::json!({})), ["build"]);
}

#[test]
fn a_package_the_page_links_by_file_is_found() {
    let dir = std::env::temp_dir()
        .join("cargo-hapto-tests")
        .join(format!("linked-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (page, hooks) = (dir.join("plugin/page"), dir.join("hapto/page"));
    std::fs::create_dir_all(&page).expect("made");
    std::fs::create_dir_all(&hooks).expect("made");
    std::fs::write(hooks.join("package.json"), "{}").expect("written");
    let package = serde_json::json!({
        "dependencies": {
            "react": "19.3.1",
            "@hapto/page": "file:../../hapto/page",
            "@gone/page": "file:../../gone",
        },
        "devDependencies": { "vite": "8.3.5" },
    });
    assert_eq!(linked(&page, &package), [page.join("../../hapto/page")]);
    assert!(linked(&page, &serde_json::json!({})).is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_map_is_written_on_a_machine_and_checked_in_ci() {
    let blesses = |ci: Option<&str>| {
        param_map_test("gain", Path::new("."), ci.map(OsStr::new))
            .get_envs()
            .any(|(key, value)| key == "HAPTO_BLESS" && value == Some(OsStr::new("1")))
    };
    assert!(blesses(None), "written where CI is not set");
    assert!(blesses(Some("")), "an empty CI is not set");
    assert!(
        !blesses(Some("true")),
        "checked in CI, which refuses a test that rewrites its own file"
    );
}

#[test]
fn a_port_something_listens_on_is_refused() {
    let held = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
    let port = held.local_addr().expect("bound").port();
    let refused = claim(port).expect_err("taken");
    assert!(refused.contains(&port.to_string()), "{refused}");
    drop(held);
    assert_eq!(claim(port), Ok(()), "free again");
}

#[test]
fn a_held_port_is_refused_before_the_page_s_packages_are_installed() {
    // A page with no packages, so an install would run npm, which a port
    // already held must not wait for.
    let page = std::env::temp_dir()
        .join("cargo-hapto-tests")
        .join(format!("held-port-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&page);
    std::fs::create_dir_all(&page).expect("made");
    let held = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
    let port = held.local_addr().expect("bound").port();
    let refused = DevServer::start(&page, port).expect_err("taken");
    assert!(refused.contains(&format!("port {port}")), "{refused}");
    drop(held);
    let _ = std::fs::remove_dir_all(page);
}
