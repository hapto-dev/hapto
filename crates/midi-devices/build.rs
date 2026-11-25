//! Sets the `hapto_backend` cfg on a system the crate has a backend for:
//! WinMM on Windows, and ALSA's sequencer on Linux, but not under Miri,
//! which cannot call into alsa-lib. What only a backend uses is built where
//! it is set, and the `none` backend where it is not.

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(hapto_backend)");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let miri = std::env::var_os("CARGO_CFG_MIRI").is_some();
    if os == "windows" || (os == "linux" && !miri) {
        println!("cargo::rustc-cfg=hapto_backend");
    }
}
