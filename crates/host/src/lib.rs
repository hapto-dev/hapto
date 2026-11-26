//! The plugin as every host sees it.
//!
//! A plugin implements [`Plugin`] once, with no knowledge of any format. An
//! exporter, a module behind the cargo feature of its name, projects it into
//! one format, as a thin shell over the core all exporters share: the `vst3`
//! module natively, the `cabi` module as a C ABI plus a generated header,
//! and the `standalone` module for an app of the plugin's own, with no host.
//! One generic JUCE adapter turns the C ABI into AU, AUv3, AAX and hosting.
//!
//! Hapto builds each instance's [`Parts`], its parameter set, session and
//! saved state, from the plugin's declarations, and hands them to
//! [`Plugin::new`]. The audio thread gets each block as a [`Block`] in
//! [`Processor::process`], whose [`Audio`] is in the processor's own
//! [layout](layout::Layout), and reads the parameters the plugin handed its
//! processor.
//!
//! What a host reads before creating an instance is [`PluginInfo`]. A plugin
//! that shows a window of its own hands out an [`Editor`] from
//! [`Plugin::editor`], which an exporter opens inside the host's.
//! [`SavedState`] is the saved block, the plugin's parameters and session as
//! one JSON object, saved and loaded from any thread but audio.
//!
//! Invariants:
//!
//! - The main thread owns the plugin ([`Plugin`] says which thread that
//!   is) and builds its [`Processor`] at activation, allocating everything
//!   the audio thread will touch up front. The audio thread then owns the
//!   processor and never allocates, locks or makes a system call, except
//!   while a panic unwinds.
//! - `unsafe` is denied workspace-wide. Only the exporters and the
//!   `boundary`, `engine` and `faults` modules they share allow it, with a
//!   `// SAFETY:` comment on every block.
//!
//! # Examples
//!
//! A plugin is one [`Plugin`] impl and one export macro at the root of its
//! crate: `export_vst3!` for a VST3 module, or `export_cabi!` for the JUCE
//! adapter's static library. [`Plugin::idle`]'s example adds undo and MIDI
//! learn.
//!
//! ```no_run
//! use hapto_host::layout::Stereo;
//! use hapto_host::{
//!     AuCodes, Block, Category, Parts, Plugin, PluginInfo, ProcessSetup, Processor, Status,
//! };
//! use hapto_params::{Declarer, Float, FloatDecl, Params};
//!
//! /// The parameters, each declared where it is held.
//! #[derive(Clone)]
//! struct GainParams {
//!     gain: Float,
//! }
//!
//! impl Params for GainParams {
//!     fn declare(p: &mut Declarer) -> Self {
//!         GainParams {
//!             gain: p.add(FloatDecl::new("gain", "Gain", -24.0..=24.0, 0.0).with_unit("dB")),
//!         }
//!     }
//! }
//!
//! /// The audio-thread half: scales both channels by the gain.
//! struct Level(GainParams);
//!
//! impl Processor for Level {
//!     type Layout = Stereo;
//!
//!     fn process(&mut self, mut block: Block<'_, Stereo>) -> Status {
//!         let gain = 10.0_f32.powf(self.0.gain.get() / 20.0);
//!         for [left, right] in block.audio.frames_mut() {
//!             *left *= gain;
//!             *right *= gain;
//!         }
//!         Status::Continue
//!     }
//! }
//!
//! /// The main-thread half: it keeps the parameters Hapto declared for it.
//! struct Gain {
//!     params: GainParams,
//! }
//!
//! impl Plugin for Gain {
//!     const INFO: PluginInfo = PluginInfo {
//!         name: "Gain",
//!         vendor: "Example",
//!         url: "",
//!         email: "",
//!         description: "",
//!         version: "0.1.0",
//!         id: "dev.example.gain",
//!         // A real plugin needs its own fixed, unique id here.
//!         vst3_class_id: [0; 16],
//!         au_codes: AuCodes { subtype: *b"gain", manufacturer: *b"Exmp" },
//!         category: Category::Effect,
//!         features: &[],
//!         // A processor in a typed layout runs at that layout's own width.
//!         layouts: &[],
//!         midi_input: false,
//!         midi_output: false,
//!     };
//!     type Params = GainParams;
//!     type Processor = Level;
//!
//!     fn new(params: GainParams, _parts: Parts) -> Self {
//!         Gain { params }
//!     }
//!
//!     // Everything the audio thread will touch is built here, on the main thread.
//!     fn activate(&mut self, _setup: &ProcessSetup) -> Level {
//!         Level(self.params.clone())
//!     }
//! }
//!
//! // At the root of the plugin's `cdylib` crate.
//! # #[cfg(all(feature = "vst3", any(windows, target_os = "macos")))]
//! hapto_host::export_vst3!(Gain);
//! ```

// The nightly real-time sanitizer lane marks the audio thread's entry points.
#![cfg_attr(hapto_rtsan, feature(sanitize))]
// docs.rs shows which feature each gated item needs.
#![cfg_attr(docsrs, feature(doc_cfg))]

mod audio;
mod editor;
mod info;
mod parts;
mod plugin;
mod spans;
mod state;

// The core every exporter shares: an exporter drives it, and nothing else
// does.
#[allow(unsafe_code, reason = "a host boundary: every block says why it holds")]
#[cfg_attr(
    not(any(feature = "vst3", feature = "cabi", feature = "standalone", test)),
    expect(dead_code, reason = "only an exporter drives the core")
)]
mod boundary;
#[cfg(feature = "cabi")]
#[cfg_attr(docsrs, doc(cfg(feature = "cabi")))]
#[allow(unsafe_code, reason = "a host boundary: every block says why it holds")]
pub mod cabi;
#[allow(unsafe_code, reason = "a host boundary: every block says why it holds")]
#[cfg_attr(
    not(any(feature = "vst3", feature = "cabi", feature = "standalone")),
    expect(dead_code, reason = "only an exporter drives the engine")
)]
mod engine;
#[allow(unsafe_code, reason = "a host boundary: every block says why it holds")]
#[cfg_attr(
    not(any(feature = "vst3", feature = "cabi", feature = "standalone")),
    expect(dead_code, reason = "only an exporter drives the core")
)]
mod faults;
#[cfg(feature = "test-host")]
#[cfg_attr(docsrs, doc(cfg(feature = "test-host")))]
pub mod play;
#[cfg(feature = "standalone")]
#[cfg_attr(docsrs, doc(cfg(feature = "standalone")))]
#[allow(unsafe_code, reason = "a host boundary: every block says why it holds")]
pub mod standalone;
#[cfg(feature = "vst3")]
#[cfg_attr(docsrs, doc(cfg(feature = "vst3")))]
#[allow(unsafe_code, reason = "a host boundary: every block says why it holds")]
pub mod vst3;

// Inlined: `Event` and `EventList` are `hapto_ump`'s, shown here as this
// crate's, so a plugin needs no `hapto_ump` import for its blocks.
#[doc(inline)]
pub use audio::{Block, Event, EventList, MAX_EVENTS, MAX_FRAMES, ProcessSetup, Status};
pub use boundary::ParentWindow;
pub use editor::{Editor, Size};
#[cfg(any(feature = "vst3", feature = "cabi", feature = "standalone"))]
pub use faults::boundary_faults;
#[doc(hidden)]
pub use faults::report_line;
// Inlined: a block's audio is `hapto_audio`'s, shown here as this crate's.
#[doc(inline)]
pub use hapto_audio::{AddSample, Audio, AudioBuf, Frame, layout};
/// Writes a plugin's [`PluginInfo`]: who the plugin is from its manifest, and
/// what its code relies on from the fields the macro is given.
///
/// It reads the manifest of the crate it is used in. Its
/// `[package.metadata.hapto]` table gives the `name`, the `vendor`, the
/// vendor's `url` and `email`, the `id`, the `vst3-class-id` written as a
/// UUID, the `au-codes`, the `category` and the `features`: each of the last
/// two is a [`Category`] or [`Feature`] variant in kebab case, `effect` or
/// `pitch-shift`. The `url`, the `email` and the `features` may be left out,
/// for none. The package's own `version` and `description` are the plugin's.
/// The fields written in Rust are the rest of `PluginInfo`'s, the MIDI flags
/// and, for a processor in [`AnyLayout`](layout::AnyLayout), the
/// [`layouts`](PluginInfo::layouts) it runs at, beside the code that relies
/// on them. Left out, `layouts` is empty, as a typed layout needs it.
///
/// Cargo reads no `[package.metadata]` table, so the macro checks this one
/// while compiling. It refuses a key the table does not take, a value of the
/// wrong type or form, a version that is not `major.minor.patch`, and a field
/// written in Rust that the manifest gives, each mistake with its line in the
/// manifest. A field written in neither place is the compiler's error, as in
/// any struct literal. A change to the manifest builds the crate again.
///
/// # Examples
///
/// This crate's own manifest holds the table the example reads:
///
/// ```toml
/// [package.metadata.hapto]
/// name = "Tone"
/// vendor = "Example"
/// url = "https://example.com"
/// id = "com.example.tone"
/// vst3-class-id = "8f3c2a71-5d0e-4b96-a1c4-7e25d9b03f68"
/// au-codes = { subtype = "tone", manufacturer = "Exmp" }
/// category = "effect"
/// features = ["equalizer"]
/// ```
///
/// ```
/// use hapto_host::layout::Width;
/// use hapto_host::{Category, Feature, PluginInfo};
///
/// const INFO: PluginInfo = hapto_host::manifest_info! {
///     layouts: &[Width::STEREO, Width::MONO],
///     midi_input: false,
///     midi_output: false,
/// };
/// assert_eq!((INFO.name, INFO.email), ("Tone", ""));
/// assert_eq!(INFO.au_codes.subtype, *b"tone");
/// assert_eq!((INFO.category, INFO.features), (Category::Effect, &[Feature::Equalizer][..]));
/// assert_eq!(INFO.version, env!("CARGO_PKG_VERSION"));
/// ```
#[cfg(feature = "manifest")]
#[cfg_attr(docsrs, doc(cfg(feature = "manifest")))]
pub use hapto_macros::manifest_info;
/// MIDI in musicians' terms: the messages that a block's [`Midi`] holds, and
/// their values.
pub use hapto_midi as midi;
pub use hapto_midi::{Message, Midi, MidiOut, Span, Spans};
pub use info::{AuCodes, Category, Feature, PluginInfo};
#[doc(hidden)]
pub use info::{first_width, widths};
pub use parts::{Driver, Maker, Parts};
pub use plugin::{Plugin, Processor, Tail};
pub use state::{LoadError, MAX_STATE_BYTES, STATE_VERSION, SavedState};
