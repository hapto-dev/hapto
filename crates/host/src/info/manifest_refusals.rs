//! The mistakes in Rust that `manifest_info!` refuses while compiling, as doc
//! tests. Those in the manifest are tested where the macro reads it, in
//! `seam-macros`, since a crate has one manifest.
//!
//! Each `compile_fail` example sits beside a twin with the mistake fixed,
//! which compiles: a `compile_fail` example passes on any error at all, so
//! the twin, and the error code where the compiler gives one, show the error
//! is the one meant. The macro's own errors have no code.

#![allow(dead_code, reason = "items that exist only to carry doc tests")]

/// A field the manifest gives, written in Rust as well.
///
/// ```compile_fail
/// use seam_host::{Layout, PluginInfo};
///
/// const INFO: PluginInfo = seam_host::manifest_info! {
///     name: "Tone",
///     layouts: &[Layout::STEREO],
///     midi_input: false,
///     midi_output: false,
/// };
/// ```
///
/// ```
/// use seam_host::{Layout, PluginInfo};
///
/// const INFO: PluginInfo = seam_host::manifest_info! {
///     layouts: &[Layout::STEREO],
///     midi_input: false,
///     midi_output: false,
/// };
/// ```
struct FieldTheManifestGives;

/// A field written in neither place.
///
/// ```compile_fail,E0063
/// use seam_host::{Layout, PluginInfo};
///
/// const INFO: PluginInfo = seam_host::manifest_info! {
///     layouts: &[Layout::STEREO],
///     midi_input: false,
/// };
/// ```
///
/// ```
/// use seam_host::{Layout, PluginInfo};
///
/// const INFO: PluginInfo = seam_host::manifest_info! {
///     layouts: &[Layout::STEREO],
///     midi_input: false,
///     midi_output: false,
/// };
/// ```
struct FieldInNeither;
