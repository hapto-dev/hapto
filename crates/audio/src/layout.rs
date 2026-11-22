//! The channels a processor's main bus has: counted at run time as a
//! [`Width`], and fixed at compile time as a [`Layout`].

use crate::Frame;

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
/// A typed layout, [`Mono`], [`Stereo`] or [`StereoOut`], fixes its channels
/// at compile time, so a frame of its audio is a sample or an array of
/// them, and a stereo frame destructures as `[left, right]`. [`AnyLayout`]
/// runs at whichever width the host chose among those the plugin lists, and
/// its frames are [`Frame`]s, counted at run time.
///
/// The trait is sealed: these four are the layouts there are.
///
/// Open: a plugin that offers several typed layouts, as a set it matches on,
/// is still to design. Until then it takes [`AnyLayout`].
///
/// # Examples
///
/// ```
/// use hapto_audio::layout::{AnyLayout, Layout, Stereo, Width};
///
/// assert_eq!(Stereo::WIDTHS, &[Width::STEREO]);
/// // AnyLayout offers no width of its own: its plugin lists them.
/// assert!(AnyLayout::WIDTHS.is_empty());
/// ```
pub trait Layout: sealed::Sealed + 'static {
    /// The widths this layout runs at, the first by default. Empty for
    /// [`AnyLayout`], which runs at the widths its plugin lists.
    const WIDTHS: &'static [Width];

    /// One frame of audio in this layout, as
    /// [`Audio::frames_mut`](crate::Audio::frames_mut) hands it out: one
    /// sample of each channel, at one moment.
    type Frame<'a>;

    /// The frame of one sample from each channel, in order; `None` when
    /// there are too few for this layout.
    #[doc(hidden)]
    fn frame<'a>(samples: impl Iterator<Item = &'a mut f32>) -> Option<Self::Frame<'a>>;

    /// Whether audio of `channels` channels fits this layout.
    #[doc(hidden)]
    fn fits(channels: usize) -> bool {
        Self::WIDTHS.is_empty()
            || Self::WIDTHS
                .iter()
                .any(|width| width.channels() == channels)
    }
}

/// One channel in, one out, processed in place: a mono effect. A frame is
/// the one sample.
///
/// # Examples
///
/// ```
/// use hapto_audio::AudioBuf;
/// use hapto_audio::layout::{Mono, Width};
///
/// let mut buffer = AudioBuf::<Mono>::new(Width::MONO, 4);
/// for sample in buffer.audio().frames_mut() {
///     *sample = 0.5;
/// }
/// assert_eq!(buffer.audio().peak(), 0.5);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mono;

/// Two channels in and two out, processed in place: a stereo effect. A
/// frame is `[left, right]`.
///
/// # Examples
///
/// ```
/// use hapto_audio::AudioBuf;
/// use hapto_audio::layout::{Stereo, Width};
///
/// let mut buffer = AudioBuf::<Stereo>::new(Width::STEREO, 4);
/// // Swaps the channels, frame by frame.
/// for [left, right] in buffer.audio().frames_mut() {
///     std::mem::swap(left, right);
/// }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stereo;

/// No input and two channels out, zeroed on entry: an instrument. A frame is
/// `[left, right]`.
///
/// # Examples
///
/// ```
/// use hapto_audio::AudioBuf;
/// use hapto_audio::layout::{StereoOut, Width};
///
/// let mut buffer = AudioBuf::<StereoOut>::new(Width::new(0, 2), 4);
/// for [left, right] in buffer.audio().frames_mut() {
///     (*left, *right) = (0.25, -0.25);
/// }
/// assert_eq!(buffer.audio().peak(), 0.25);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StereoOut;

/// Whichever width the host chose among those the plugin lists, counted at
/// run time. A frame is a [`Frame`] of every channel.
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

impl Layout for Mono {
    const WIDTHS: &'static [Width] = &[Width::MONO];

    type Frame<'a> = &'a mut f32;

    fn frame<'a>(mut samples: impl Iterator<Item = &'a mut f32>) -> Option<&'a mut f32> {
        samples.next()
    }
}

impl Layout for Stereo {
    const WIDTHS: &'static [Width] = &[Width::STEREO];

    type Frame<'a> = [&'a mut f32; 2];

    fn frame<'a>(mut samples: impl Iterator<Item = &'a mut f32>) -> Option<[&'a mut f32; 2]> {
        Some([samples.next()?, samples.next()?])
    }
}

impl Layout for StereoOut {
    const WIDTHS: &'static [Width] = &[Width::new(0, 2)];

    type Frame<'a> = [&'a mut f32; 2];

    fn frame<'a>(mut samples: impl Iterator<Item = &'a mut f32>) -> Option<[&'a mut f32; 2]> {
        Some([samples.next()?, samples.next()?])
    }
}

impl Layout for AnyLayout {
    const WIDTHS: &'static [Width] = &[];

    type Frame<'a> = Frame<'a>;

    fn frame<'a>(samples: impl Iterator<Item = &'a mut f32>) -> Option<Frame<'a>> {
        Some(Frame::of(samples))
    }
}

mod sealed {
    /// Keeps [`Layout`](super::Layout) to the layouts this crate defines.
    pub trait Sealed {}

    impl Sealed for super::Mono {}
    impl Sealed for super::Stereo {}
    impl Sealed for super::StereoOut {}
    impl Sealed for super::AnyLayout {}
}
