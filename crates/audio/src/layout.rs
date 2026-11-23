//! The channels a processor's main bus has: counted at run time as a
//! [`Width`], and fixed at compile time as a [`Layout`].

use std::iter::Take;

use crate::Frame;
use crate::audio::{AnyFrames, BusFrames};

/// The most channels audio holds, and so the widest [`Width`] a plugin may
/// offer: 16, enough for a 9.1.6 layout. A plugin that offers a wider one
/// fails to build.
pub const MAX_CHANNELS: usize = 16;

/// The channels a main bus has in and out, counted at run time: what a host
/// chooses among those a plugin offers.
///
/// An instrument has no input, so its `inputs` is 0. A block's audio holds
/// [`channels`](Self::channels) of them, processed in place.
///
/// # Examples
///
/// ```
/// use hapto_audio::layout::Width;
///
/// // A plugin that runs as stereo or as mono; the first is the default.
/// const WIDTHS: &[Width] = &[Width::STEREO, Width::MONO];
/// assert_eq!(WIDTHS[0], Width::new(2, 2));
/// // An instrument's two outputs are two channels of audio.
/// assert_eq!(Width::new(0, 2).channels(), 2);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Width {
    /// Input channels.
    pub inputs: usize,
    /// Output channels.
    pub outputs: usize,
}

impl Width {
    /// One channel in, one out.
    pub const MONO: Width = Width::new(1, 1);
    /// Two channels in, two out.
    pub const STEREO: Width = Width::new(2, 2);

    /// Creates a width of `inputs` channels in and `outputs` out.
    pub const fn new(inputs: usize, outputs: usize) -> Self {
        Width { inputs, outputs }
    }

    /// The channels a block's audio holds at this width: the larger count,
    /// since the audio is processed in place.
    pub const fn channels(self) -> usize {
        if self.inputs > self.outputs {
            self.inputs
        } else {
            self.outputs
        }
    }
}

/// The channels a processor's main bus has, as a type it names as its
/// `Processor::Layout`.
///
/// A [`Bus`] fixes its channels at compile time, so its frames and its
/// channels are arrays, which destructure. [`AnyLayout`] runs at whichever
/// width the host chose among those the plugin lists, and hands out its
/// frames and channels counted at run time.
///
/// The trait is sealed: a `Bus` of any width up to [`MAX_CHANNELS`] each way,
/// and `AnyLayout`, are the layouts there are.
///
/// Open: a plugin that offers several typed layouts, as a set it matches on,
/// is still to design. Until then it takes [`AnyLayout`].
///
/// # Examples
///
/// ```
/// use hapto_audio::layout::{AnyLayout, Bus, Layout, Stereo, Width};
///
/// assert_eq!(Stereo::WIDTHS, &[Width::STEREO]);
/// assert_eq!(Bus::<1, 2>::WIDTHS, &[Width::new(1, 2)]);
/// // AnyLayout offers no width of its own: its plugin lists them.
/// assert!(AnyLayout::WIDTHS.is_empty());
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a layout",
    note = "a `Bus` has at most 16 channels in and 16 out; a plugin that runs \
            at several widths takes `AnyLayout`"
)]
pub trait Layout: sealed::Sealed + 'static {
    /// The widths this layout runs at, the first by default. Empty for
    /// [`AnyLayout`], which runs at the widths its plugin lists.
    const WIDTHS: &'static [Width];

    /// One frame of audio in this layout, as
    /// [`Audio::frames_mut`](crate::Audio::frames_mut) hands it out: one
    /// sample of each channel, at one moment.
    type Frame<'a>;

    /// The channels, as [`Audio::channels`](crate::Audio::channels) hands
    /// them out: an array in a `Bus`, an iterator in `AnyLayout`.
    type Channels<'a>: IntoIterator<Item = &'a [f32]>;

    /// The channels, as
    /// [`Audio::channels_mut`](crate::Audio::channels_mut) hands them out:
    /// an array in a `Bus`, an iterator in `AnyLayout`.
    type ChannelsMut<'a>: IntoIterator<Item = &'a mut [f32]>;

    /// The frames of the first `count` of `channels`, each `frames` long, as
    /// [`Audio::frames_mut`](crate::Audio::frames_mut) walks them.
    #[doc(hidden)]
    fn frames_of(
        channels: [&mut [f32]; MAX_CHANNELS],
        count: usize,
        frames: usize,
    ) -> impl ExactSizeIterator<Item = Self::Frame<'_>>;

    /// The first `count` of `channels`, as this layout hands them out.
    #[doc(hidden)]
    fn channels_of(channels: [&[f32]; MAX_CHANNELS], count: usize) -> Self::Channels<'_>;

    /// The first `count` of `channels`, as this layout hands them out.
    #[doc(hidden)]
    fn channels_mut_of(channels: [&mut [f32]; MAX_CHANNELS], count: usize)
    -> Self::ChannelsMut<'_>;

    /// Whether audio of `channels` channels fits this layout.
    #[doc(hidden)]
    fn fits(channels: usize) -> bool {
        Self::WIDTHS.is_empty()
            || Self::WIDTHS
                .iter()
                .any(|width| width.channels() == channels)
    }
}

/// A main bus of `IN` channels in and `OUT` out, fixed at compile time. Its
/// audio holds the larger count, processed in place: on entry the first `IN`
/// carry the input and any further ones are zeroed.
///
/// Its frames and its channels are arrays, so they destructure:
/// `for [left, right] in audio.frames_mut()` walks a stereo block frame by
/// frame, and `let [left, right] = audio.channels_mut()` reaches each channel
/// by its place, with no index and nothing to unwrap. A pattern that names a
/// channel the bus does not have fails to compile.
///
/// It is a [`Layout`] for every count up to [`MAX_CHANNELS`] each way. The
/// common ones have names: [`Mono`], [`Stereo`], [`StereoOut`],
/// [`MonoToStereo`] and [`NoAudio`].
///
/// # Examples
///
/// Four channels in and out, with a gain on the third alone:
///
/// ```
/// use hapto_audio::AudioBuf;
/// use hapto_audio::layout::Bus;
///
/// let mut buffer = AudioBuf::<Bus<4, 4>>::new(8);
/// let mut audio = buffer.audio();
/// audio.add_to_all([1.0; 8]);
/// let [_, _, third, _] = audio.channels_mut();
/// for sample in third {
///     *sample *= 0.5;
/// }
/// let [first, _, third, _] = buffer.channels();
/// assert_eq!((first[0], third[0]), (1.0, 0.5));
/// ```
///
/// A third channel of a stereo bus does not compile:
///
/// ```compile_fail,E0527
/// use hapto_audio::AudioBuf;
/// use hapto_audio::layout::Stereo;
///
/// let buffer = AudioBuf::<Stereo>::new(8);
/// let [_, _, third] = buffer.channels();
/// ```
///
/// Nor does a bus wider than [`MAX_CHANNELS`]:
///
/// ```compile_fail,E0277
/// use hapto_audio::AudioBuf;
/// use hapto_audio::layout::Bus;
///
/// let buffer = AudioBuf::<Bus<17, 2>>::new(8);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bus<const IN: usize, const OUT: usize>;

/// One channel in, one out: a mono effect. A frame is `[sample]`.
///
/// # Examples
///
/// ```
/// use hapto_audio::AudioBuf;
/// use hapto_audio::layout::Mono;
///
/// let mut buffer = AudioBuf::<Mono>::new(4);
/// for [sample] in buffer.audio().frames_mut() {
///     *sample = 0.5;
/// }
/// assert_eq!(buffer.audio().peak(), 0.5);
/// ```
pub type Mono = Bus<1, 1>;

/// Two channels in and two out: a stereo effect. A frame is
/// `[left, right]`.
///
/// # Examples
///
/// ```
/// use hapto_audio::AudioBuf;
/// use hapto_audio::layout::Stereo;
///
/// let mut buffer = AudioBuf::<Stereo>::new(4);
/// // Swaps the channels, frame by frame.
/// for [left, right] in buffer.audio().frames_mut() {
///     std::mem::swap(left, right);
/// }
/// ```
pub type Stereo = Bus<2, 2>;

/// No input and two channels out, zeroed on entry: an instrument. A frame is
/// `[left, right]`.
///
/// # Examples
///
/// ```
/// use hapto_audio::AudioBuf;
/// use hapto_audio::layout::StereoOut;
///
/// let mut buffer = AudioBuf::<StereoOut>::new(4);
/// for [left, right] in buffer.audio().frames_mut() {
///     (*left, *right) = (0.25, -0.25);
/// }
/// assert_eq!(buffer.audio().peak(), 0.25);
/// ```
pub type StereoOut = Bus<0, 2>;

/// One channel in and two out: a reverb or a widener on a mono track. Its
/// audio holds two channels; on entry the first carries the input and the
/// second is zeroed.
///
/// # Examples
///
/// ```
/// use hapto_audio::AudioBuf;
/// use hapto_audio::layout::MonoToStereo;
///
/// let mut buffer = AudioBuf::<MonoToStereo>::new(4);
/// // Spreads the input across both outputs.
/// for [left, right] in buffer.audio().frames_mut() {
///     let input = *left;
///     (*left, *right) = (0.7 * input, 0.7 * input);
/// }
/// ```
pub type MonoToStereo = Bus<1, 2>;

/// No audio at all: a plugin that only handles MIDI. Its audio has no
/// channels, and its frames still count the block's time.
///
/// # Examples
///
/// ```
/// use hapto_audio::AudioBuf;
/// use hapto_audio::layout::NoAudio;
///
/// let mut buffer = AudioBuf::<NoAudio>::new(64);
/// assert_eq!(buffer.audio().frames_mut().count(), 64);
/// ```
pub type NoAudio = Bus<0, 0>;

/// Whichever width the host chose among those the plugin lists, counted at
/// run time. A frame is a [`Frame`] of every channel, and the channels come
/// as an iterator.
///
/// What a plugin that offers several widths takes, and what code that runs
/// in any layout, such as a voice, writes into.
///
/// # Examples
///
/// ```
/// use hapto_audio::{Audio, AudioBuf};
///
/// /// Adds a mono tone into every channel, whatever the width.
/// fn tone(out: &mut Audio<'_>) {
///     for frame in out.frames_mut() {
///         frame.add(0.1);
///     }
/// }
///
/// let mut buffer = AudioBuf::stereo(4);
/// tone(&mut buffer.audio());
/// assert!(buffer.channels().all(|channel| channel == [0.1; 4]));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnyLayout;

impl Layout for AnyLayout {
    const WIDTHS: &'static [Width] = &[];

    type Frame<'a> = Frame<'a>;
    type Channels<'a> = Take<std::array::IntoIter<&'a [f32], MAX_CHANNELS>>;
    type ChannelsMut<'a> = Take<std::array::IntoIter<&'a mut [f32], MAX_CHANNELS>>;

    fn frames_of(
        channels: [&mut [f32]; MAX_CHANNELS],
        count: usize,
        frames: usize,
    ) -> impl ExactSizeIterator<Item = Frame<'_>> {
        AnyFrames::new(channels, count, frames)
    }

    fn channels_of(channels: [&[f32]; MAX_CHANNELS], count: usize) -> Self::Channels<'_> {
        channels.into_iter().take(count)
    }

    fn channels_mut_of(
        channels: [&mut [f32]; MAX_CHANNELS],
        count: usize,
    ) -> Self::ChannelsMut<'_> {
        channels.into_iter().take(count)
    }
}

/// The channels a bus's audio holds: the larger of its counts.
const fn held(inputs: usize, outputs: usize) -> usize {
    Width::new(inputs, outputs).channels()
}

/// The first `N` of `channels`, which hold at least `N`: a layout's count
/// is checked when its audio is made.
fn first_of<T: Default, const N: usize>(channels: [T; MAX_CHANNELS]) -> [T; N] {
    let mut channels = channels.into_iter();
    std::array::from_fn(|_| channels.next().unwrap_or_default())
}

/// A [`Bus`] layout for every count of inputs and outputs listed, each pair
/// with its own array lengths, since stable Rust cannot compute one from a
/// generic count.
macro_rules! buses {
    ($($count:literal)*) => {
        buses!(@rows [$($count)*] [$($count)*]);
    };
    (@rows [$($inputs:literal)*] $outputs:tt) => {
        $(buses!(@row $inputs $outputs);)*
    };
    (@row $inputs:literal [$($outputs:literal)*]) => {$(
        impl sealed::Sealed for Bus<$inputs, $outputs> {}

        #[doc(hidden)]
        impl Layout for Bus<$inputs, $outputs> {
            const WIDTHS: &'static [Width] = &[Width::new($inputs, $outputs)];

            type Frame<'a> = [&'a mut f32; held($inputs, $outputs)];
            type Channels<'a> = [&'a [f32]; held($inputs, $outputs)];
            type ChannelsMut<'a> = [&'a mut [f32]; held($inputs, $outputs)];

            fn frames_of(
                channels: [&mut [f32]; MAX_CHANNELS],
                _: usize,
                frames: usize,
            ) -> impl ExactSizeIterator<Item = Self::Frame<'_>> {
                BusFrames::<{ held($inputs, $outputs) }>::new(first_of(channels), frames)
            }

            fn channels_of(channels: [&[f32]; MAX_CHANNELS], _: usize) -> Self::Channels<'_> {
                first_of(channels)
            }

            fn channels_mut_of(
                channels: [&mut [f32]; MAX_CHANNELS],
                _: usize,
            ) -> Self::ChannelsMut<'_> {
                first_of(channels)
            }
        }
    )*};
}

buses!(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16);

mod sealed {
    /// Keeps [`Layout`](super::Layout) to the layouts this crate defines.
    pub trait Sealed {}

    impl Sealed for super::AnyLayout {}
}
