//! Sets the `hapto_backend` cfg where the crate has a backend, which is
//! WinMM on Windows and ALSA's sequencer on Linux except under Miri, because
//! Miri can't call into alsa-lib.
//!
//! Code that only a backend uses is built where the cfg is set, and the
//! `none` backend where it isn't.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(hapto_backend)");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let miri = std::env::var_os("CARGO_CFG_MIRI").is_some();
    if os == "windows" || (os == "linux" && !miri) {
        println!("cargo::rustc-cfg=hapto_backend");
    }
}
