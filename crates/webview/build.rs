//! Sets the `hapto_webview` cfg on the systems where the editor has a web
//! engine, which today is Windows with WebView2. The editor and the code that
//! only it uses are built where the cfg is set. Elsewhere a page offers no
//! editor.
//!
//! The WebView library's dependency table in Cargo.toml lists the same
//! systems, because Cargo can't read a cfg that a build script sets.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(hapto_webview)");
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        println!("cargo::rustc-cfg=hapto_webview");
    }
}
