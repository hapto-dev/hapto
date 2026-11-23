//! The mistakes in Rust that `manifest_info!` refuses while compiling, as doc
//! tests. Those in the manifest are tested where the macro reads it, in
//! `hapto-macros`, since a crate has one manifest.
//!
//! Each `compile_fail` example sits beside a twin with the mistake fixed,
//! which compiles: a `compile_fail` example passes on any error at all, so
//! the twin, and the error code where the compiler gives one, show the error
//! is the one meant. The macro's own errors have no code.

#![allow(dead_code, reason = "items that exist only to carry doc tests")]

/// A field the manifest gives, written in Rust as well.
///
/// ```compile_fail
/// use hapto_host::PluginInfo;
/// use hapto_host::layout::Width;
///
/// const INFO: PluginInfo = hapto_host::manifest_info! {
///     name: "Tone",
///     layouts: &[Width::STEREO],
///     midi_input: false,
///     midi_output: false,
/// };
/// ```
///
/// ```
/// use hapto_host::PluginInfo;
/// use hapto_host::layout::Width;
///
/// const INFO: PluginInfo = hapto_host::manifest_info! {
///     layouts: &[Width::STEREO],
///     midi_input: false,
///     midi_output: false,
/// };
/// ```
struct FieldTheManifestGives;

/// A field written in neither place.
///
/// ```compile_fail,E0063
/// use hapto_host::PluginInfo;
/// use hapto_host::layout::Width;
///
/// const INFO: PluginInfo = hapto_host::manifest_info! {
///     layouts: &[Width::STEREO],
///     midi_input: false,
/// };
/// ```
///
/// ```
/// use hapto_host::PluginInfo;
/// use hapto_host::layout::Width;
///
/// const INFO: PluginInfo = hapto_host::manifest_info! {
///     layouts: &[Width::STEREO],
///     midi_input: false,
///     midi_output: false,
/// };
/// ```
///
/// `layouts` is the one field that may be left out: it is empty then, as a
/// processor in a typed layout needs it.
///
/// ```
/// use hapto_host::PluginInfo;
///
/// const INFO: PluginInfo = hapto_host::manifest_info! {
///     midi_input: false,
///     midi_output: false,
/// };
/// assert!(INFO.layouts.is_empty());
/// ```
struct FieldInNeither;
