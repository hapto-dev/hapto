//! Sets the `hapto_app` cfg on each system that the app runs on. The app,
//! its device and MIDI code, and the dev host are built where the cfg is set,
//! and elsewhere `main` says that the app doesn't run there.
//!
//! The app's dependency table in Cargo.toml must name the same systems. The
//! script passes the list to the crate as `HAPTO_APP_SYSTEMS`, and a test
//! checks the table against it.

/// The systems that the app runs on, as `CARGO_CFG_TARGET_OS` names them.
const SYSTEMS: &[&str] = &["windows"];

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(hapto_app)");
    println!("cargo::rustc-env=HAPTO_APP_SYSTEMS={}", SYSTEMS.join(","));
    let os = std::env::var("CARGO_CFG_TARGET_OS").expect("Cargo sets CARGO_CFG_TARGET_OS");
    if SYSTEMS.contains(&os.as_str()) {
        println!("cargo::rustc-cfg=hapto_app");
    }
}
