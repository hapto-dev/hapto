//! What a host reads about a plugin before creating it.

#[cfg(all(doctest, feature = "manifest"))]
mod manifest_refusals;
#[cfg(test)]
mod offered;

use crate::layout::{Layout, MAX_CHANNELS, Width};
use crate::{Plugin, Processor};

/// What kind of plugin it is.
///
/// # Examples
///
/// ```
/// use hapto_host::Category;
/// use hapto_host::layout::{Layout, StereoOut};
///
/// // An instrument has no input.
/// let (category, width) = (Category::Instrument, StereoOut::WIDTHS[0]);
/// assert_eq!((category, width.inputs), (Category::Instrument, 0));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Processes audio.
    Effect,
    /// Makes sound from notes.
    Instrument,
}

/// What a plugin does, finer than its [`Category`]: each exporter lists it
/// in its format's own words, as VST3's subcategories, and as CLAP's
/// features once the planned CLAP exporter is built. More join as plugins
/// need them.
///
/// # Examples
///
/// ```
/// use hapto_host::{Category, Feature};
///
/// // What the example EQ tells a host's plugin browser.
/// const CATEGORY: Category = Category::Effect;
/// const FEATURES: &[Feature] = &[Feature::Equalizer, Feature::Filter];
/// assert_eq!(FEATURES.len(), 2);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Feature {
    /// Measures or displays the signal.
    Analyzer,
    /// Echoes.
    Delay,
    /// Saturation, overdrive, distortion.
    Distortion,
    /// Compressors, limiters, gates, expanders.
    Dynamics,
    /// An equalizer.
    Equalizer,
    /// A filter.
    Filter,
    /// For the master bus.
    Mastering,
    /// Chorus, flanger, phaser, tremolo.
    Modulation,
    /// Changes pitch.
    PitchShift,
    /// Removes noise, clicks or hum.
    Restoration,
    /// Reverb.
    Reverb,
    /// Places sound in space.
    Spatial,
    /// A utility: gain, routing, metering helpers.
    Tools,
    /// Drum sounds.
    Drum,
    /// A piano.
    Piano,
    /// Plays samples.
    Sampler,
    /// A synthesizer.
    Synthesizer,
}

/// The widths a plugin's main bus runs at, the first by default: its
/// processor's [layout](Processor::Layout)'s own, or, for a processor in
/// [`AnyLayout`](crate::layout::AnyLayout), the
/// [`layouts`](PluginInfo::layouts) its info lists.
///
/// Evaluated in a `const` block, as every exporter does, it fails to compile
/// a plugin that lists widths beside a typed layout, lists none for
/// `AnyLayout`, or offers a width of more than [`MAX_CHANNELS`] channels.
///
/// # Panics
///
/// For such a plugin, which is the point.
#[doc(hidden)]
pub const fn widths<P: Plugin>() -> &'static [Width] {
    let typed = <<P::Processor as Processor>::Layout as Layout>::WIDTHS;
    let listed = P::INFO.layouts;
    let widths = match (typed.is_empty(), listed.is_empty()) {
        (false, true) => typed,
        (true, false) => listed,
        (false, false) => panic!(
            "a processor in a typed layout runs at that layout's own width: \
             list `layouts` only for a processor in `AnyLayout`"
        ),
        (true, true) => panic!(
            "a processor in `AnyLayout` runs at the widths its plugin lists: \
             give `layouts` at least one"
        ),
    };
    let mut index = 0;
    while index < widths.len() {
        if widths[index].channels() > MAX_CHANNELS {
            panic!("a plugin offers a width of more than the 16 channels audio holds");
        }
        index += 1;
    }
    widths
}

/// The width an instance starts at, the first its plugin offers (see
/// [`widths`]). Evaluated in a `const` block, it fails to compile as
/// `widths` does.
///
/// # Panics
///
/// For a plugin `widths` refuses, which is the point.
#[doc(hidden)]
pub const fn first_width<P: Plugin>() -> Width {
    match widths::<P>() {
        [first, ..] => *first,
        [] => panic!("a plugin offers at least one width"),
    }
}

/// A plugin's two Audio Unit codes, four characters each.
///
/// The JUCE adapter's build also takes them as its plugin and manufacturer
/// codes, from which JUCE derives that build's VST3 class id. The fields are
/// named, so the two cannot be swapped unseen.
///
/// # Examples
///
/// ```
/// use hapto_host::AuCodes;
///
/// // Each plugin's own subtype, under the vendor's one manufacturer code.
/// const TONE: AuCodes = AuCodes { subtype: *b"tone", manufacturer: *b"Exmp" };
/// const GATE: AuCodes = AuCodes { subtype: *b"gate", ..TONE };
/// assert_eq!(GATE.manufacturer, TONE.manufacturer);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuCodes {
    /// The plugin's own code, unique among its manufacturer's plugins.
    pub subtype: [u8; 4],
    /// The manufacturer's code, the same for all of its plugins.
    pub manufacturer: [u8; 4],
}

/// A plugin's identity and description, the same for every format.
///
/// A plugin's [`Plugin::INFO`](crate::Plugin::INFO) is one of these, a
/// constant, written by hand as below or by `manifest_info!`, under the
/// `manifest` feature, from the plugin's manifest.
///
/// # Examples
///
/// ```
/// use hapto_host::layout::Width;
/// use hapto_host::{AuCodes, Category, Feature, PluginInfo};
///
/// const INFO: PluginInfo = PluginInfo {
///     name: "Tone",
///     vendor: "Example",
///     url: "https://example.com",
///     email: "",
///     description: "A two-band equalizer.",
///     version: "0.1.0",
///     id: "com.example.tone",
///     // A version 4 UUID, made once for this plugin and never changed.
///     vst3_class_id: 0x8f3c_2a71_5d0e_4b96_a1c4_7e25_d9b0_3f68_u128.to_be_bytes(),
///     au_codes: AuCodes { subtype: *b"tone", manufacturer: *b"Exmp" },
///     category: Category::Effect,
///     features: &[Feature::Equalizer],
///     // For a processor in `AnyLayout`: stereo by default, or mono.
///     layouts: &[Width::STEREO, Width::MONO],
///     midi_input: true,
///     midi_output: false,
/// };
/// assert_eq!(INFO.layouts[0], Width::STEREO);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginInfo {
    /// The name hosts list.
    pub name: &'static str,
    /// Who makes it.
    pub vendor: &'static str,
    /// The vendor's web address; empty for none.
    pub url: &'static str,
    /// The vendor's contact address; empty for none.
    pub email: &'static str,
    /// One line on what it does; empty for none.
    pub description: &'static str,
    /// Its version, `major.minor.patch`.
    pub version: &'static str,
    /// A reverse-DNS id, unique to this plugin (`com.vendor.product`): the
    /// JUCE adapter's bundle id, and the id a CLAP build will take. No other
    /// format's id derives from it.
    pub id: &'static str,
    /// The VST3 class id: 16 bytes no other plugin uses.
    ///
    /// Make it once from 16 random bytes, such as a version 4 UUID's, and
    /// never change it after release: a host treats a plugin with a new id as
    /// a different plugin, and its sessions lose it.
    pub vst3_class_id: [u8; 16],
    /// The AU codes: the plugin's subtype and its manufacturer's.
    pub au_codes: AuCodes,
    /// What kind of plugin it is.
    pub category: Category,
    /// What it does, finer: for a host's browser.
    pub features: &'static [Feature],
    /// For a processor in [`AnyLayout`](crate::layout::AnyLayout), the
    /// widths its main bus runs at, the first by default; empty for a typed
    /// layout, which runs at its own. A host picks one before activation,
    /// and [`ProcessSetup::layout`](crate::ProcessSetup::layout) reports it.
    ///
    /// The VST3 exporter arranges a bus of one channel as mono and of two as
    /// stereo, and refuses to build a plugin that offers a wider one. The C
    /// interface takes any width up to [`MAX_CHANNELS`].
    pub layouts: &'static [Width],
    /// Whether it takes MIDI input (for MIDI learn, or notes).
    ///
    /// Open: no exporter carries MIDI either way yet. The VST3 exporter shows
    /// a host an event bus for each direction a plugin declares, but a
    /// block's [`events`](crate::Block::events) stay empty, and what a
    /// processor pushes to [`out_events`](crate::Block::out_events) reaches
    /// no host.
    pub midi_input: bool,
    /// Whether it sends MIDI (a MIDI effect, or MIDI-CI replies), through
    /// [`Block::out_events`](crate::Block::out_events).
    ///
    /// Open: no exporter sends it yet, as [`midi_input`](Self::midi_input)
    /// says.
    pub midi_output: bool,
}

impl PluginInfo {
    /// Its VST3 subcategories, in the SDK's words: its category, then each
    /// feature ("Fx", "EQ"). The VST3 exporter joins them with `|`, and the
    /// JUCE adapter's build hands them to JUCE, which does the same.
    #[cfg(any(feature = "vst3", feature = "cabi"))]
    pub(crate) fn vst3_categories(&self) -> Vec<&'static str> {
        let first = match self.category {
            Category::Effect => "Fx",
            Category::Instrument => "Instrument",
        };
        std::iter::once(first)
            .chain(
                self.features
                    .iter()
                    .map(|&feature| vst3_subcategory(feature, self.category)),
            )
            .collect()
    }
}

/// The VST3 subcategory of a feature, in a plugin of `category`.
#[cfg(any(feature = "vst3", feature = "cabi"))]
fn vst3_subcategory(feature: Feature, category: Category) -> &'static str {
    match feature {
        Feature::Analyzer => "Analyzer",
        Feature::Delay => "Delay",
        Feature::Distortion => "Distortion",
        Feature::Dynamics => "Dynamics",
        Feature::Equalizer => "EQ",
        Feature::Filter => "Filter",
        Feature::Mastering => "Mastering",
        Feature::Modulation => "Modulation",
        Feature::PitchShift => "Pitch Shift",
        Feature::Restoration => "Restoration",
        Feature::Reverb => "Reverb",
        Feature::Spatial => "Spatial",
        Feature::Tools => "Tools",
        // The SDK's names differ: `Fx|Drums` and `Instrument|Drum`.
        Feature::Drum if category == Category::Effect => "Drums",
        Feature::Drum => "Drum",
        Feature::Piano => "Piano",
        Feature::Sampler => "Sampler",
        Feature::Synthesizer => "Synth",
    }
}

#[cfg(all(test, any(feature = "vst3", feature = "cabi")))]
mod tests {
    use super::*;

    #[test]
    fn drums_are_named_as_the_sdk_names_them() {
        assert_eq!(vst3_subcategory(Feature::Drum, Category::Effect), "Drums");
        assert_eq!(
            vst3_subcategory(Feature::Drum, Category::Instrument),
            "Drum"
        );
        assert_eq!(vst3_subcategory(Feature::Equalizer, Category::Effect), "EQ");
    }
}
