//! Sets the crate's cfgs, so each condition is named once:
//!
//! - `hapto_webview` on each system where the editor has a web engine. The
//!   editor and the code that only it uses are built where it is set, and
//!   elsewhere a page offers no editor. It is not set under Miri, which can't
//!   call into a web engine.
//! - `hapto_helper` on Linux when the `helper` feature builds the page's
//!   helper process.
//! - `hapto_wry` wherever this crate's code runs wry: in the editor on
//!   Windows, and in the helper on Linux.
//! - `hapto_webkitgtk` where the editor shows the page through the helper,
//!   which is on Linux wherever `hapto_webview` is set.
//!
//! The web engines' dependency tables in Cargo.toml must name the systems in
//! `SYSTEMS`. The script passes the list to the crate as
//! `HAPTO_WEBVIEW_SYSTEMS`, and a test checks the tables against it.

/// The systems that have a web engine, as `CARGO_CFG_TARGET_OS` names them.
const SYSTEMS: &[&str] = &["windows", "linux"];

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(hapto_webview, hapto_helper, hapto_wry, hapto_webkitgtk)");
    println!(
        "cargo::rustc-env=HAPTO_WEBVIEW_SYSTEMS={}",
        SYSTEMS.join(",")
    );
    let os = std::env::var("CARGO_CFG_TARGET_OS").expect("Cargo sets CARGO_CFG_TARGET_OS");
    let webview = SYSTEMS.contains(&os.as_str()) && std::env::var_os("CARGO_CFG_MIRI").is_none();
    let helper = os == "linux" && std::env::var_os("CARGO_FEATURE_HELPER").is_some();
    if webview {
        println!("cargo::rustc-cfg=hapto_webview");
    }
    if helper {
        println!("cargo::rustc-cfg=hapto_helper");
    }
    if (webview && os == "windows") || helper {
        println!("cargo::rustc-cfg=hapto_wry");
    }
    if webview && os == "linux" {
        println!("cargo::rustc-cfg=hapto_webkitgtk");
    }
}
