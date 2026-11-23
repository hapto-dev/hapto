//! The widths a plugin offers: a typed layout's own, or those its info
//! lists for `AnyLayout`, and the three plugins `widths` refuses. An
//! exporter evaluates `widths` while compiling, where its panic fails the
//! build; called here at run time, the same panic is the test's.

use std::marker::PhantomData;

use hapto_params::{Declarer, Params};

use super::{AuCodes, Category, PluginInfo, first_width, widths};
use crate::layout::{AnyLayout, Bus, Layout, MAX_CHANNELS, Stereo, StereoOut, Width};
use crate::{Block, Parts, Plugin, ProcessSetup, Processor, Status};

/// The widths a test plugin's info lists.
trait Lists: 'static {
    const LAYOUTS: &'static [Width];
}

/// None, as a typed layout needs.
struct Nothing;

impl Lists for Nothing {
    const LAYOUTS: &'static [Width] = &[];
}

/// Stereo by default, or mono.
struct StereoOrMono;

impl Lists for StereoOrMono {
    const LAYOUTS: &'static [Width] = &[Width::STEREO, Width::MONO];
}

/// A width audio holds, then one of a channel more than it holds.
struct TooWide;

impl Lists for TooWide {
    const LAYOUTS: &'static [Width] = &[Width::STEREO, Width::new(2, MAX_CHANNELS + 1)];
}

struct NoParams;

impl Params for NoParams {
    fn declare(_: &mut Declarer) -> Self {
        NoParams
    }
}

/// A processor in layout `L`.
struct Runs<L>(PhantomData<fn() -> L>);

impl<L: Layout> Processor for Runs<L> {
    type Layout = L;

    fn process(&mut self, _: Block<'_, L>) -> Status {
        Status::Sleep
    }
}

/// A plugin whose processor is in layout `L` and whose info lists `W`.
struct Offers<L, W>(PhantomData<fn() -> (L, W)>);

impl<L: Layout, W: Lists> Plugin for Offers<L, W> {
    const INFO: PluginInfo = PluginInfo {
        name: "Offers",
        vendor: "Hapto",
        url: "",
        email: "",
        description: "",
        version: "0.1.0",
        id: "dev.hapto.offers",
        vst3_class_id: [0; 16],
        au_codes: AuCodes {
            subtype: *b"offr",
            manufacturer: *b"Hapt",
        },
        category: Category::Effect,
        features: &[],
        layouts: W::LAYOUTS,
        midi_input: false,
        midi_output: false,
    };
    type Params = NoParams;
    type Processor = Runs<L>;

    fn new(_: NoParams, _: Parts) -> Self {
        Offers(PhantomData)
    }

    fn activate(&mut self, _: &ProcessSetup) -> Runs<L> {
        Runs(PhantomData)
    }
}

#[test]
fn a_typed_layout_offers_its_own_width() {
    assert_eq!(widths::<Offers<StereoOut, Nothing>>(), [Width::new(0, 2)]);
    assert_eq!(
        first_width::<Offers<Bus<4, 4>, Nothing>>(),
        Width::new(4, 4)
    );
}

#[test]
fn any_layout_offers_the_widths_its_plugin_lists_the_first_by_default() {
    assert_eq!(
        widths::<Offers<AnyLayout, StereoOrMono>>(),
        [Width::STEREO, Width::MONO]
    );
    assert_eq!(
        first_width::<Offers<AnyLayout, StereoOrMono>>(),
        Width::STEREO
    );
}

#[test]
#[should_panic(expected = "list `layouts` only for a processor in `AnyLayout`")]
fn widths_listed_beside_a_typed_layout_are_refused() {
    let _ = widths::<Offers<Stereo, StereoOrMono>>();
}

#[test]
#[should_panic(expected = "give `layouts` at least one")]
fn any_layout_with_no_width_listed_is_refused() {
    let _ = widths::<Offers<AnyLayout, Nothing>>();
}

#[test]
#[should_panic(expected = "more than the 16 channels audio holds")]
fn a_width_of_more_channels_than_audio_holds_is_refused() {
    let _ = widths::<Offers<AnyLayout, TooWide>>();
}
