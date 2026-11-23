//! The audio a block lends a processor.

use std::fmt;
use std::marker::PhantomData;

use crate::Frame;
use crate::layout::{AnyLayout, Layout, MAX_CHANNELS};

/// A block's channels over the same frames, lent for one call: the host's,
/// or an [`AudioBuf`](crate::AudioBuf)'s.
///
/// Its loops cannot go out of range, so audio code needs no index, no frame
/// count and no bounds check. [`frames_mut`](Self::frames_mut) walks it a
/// moment at a time across every channel, and
/// [`channels_mut`](Self::channels_mut) a channel at a time. In a
/// [`Bus`](crate::layout::Bus) both are arrays, which destructure. Hapto's
/// examples put the iterators in the loop header and the DSP in the loop
/// body.
///
/// It is named after the field of the block that holds it, and rustdoc's
/// search also finds it under JUCE's name.
///
/// # Examples
///
/// A stereo widener: in a typed layout a frame destructures.
///
/// ```
/// use hapto_audio::layout::Stereo;
/// use hapto_audio::{Audio, AudioBuf};
///
/// fn widen(audio: &mut Audio<'_, Stereo>, width: f32) {
///     for [left, right] in audio.frames_mut() {
///         let mid = 0.5 * (*left + *right);
///         let side = 0.5 * (*left - *right) * width;
///         *left = mid + side;
///         *right = mid - side;
///     }
/// }
///
/// let mut buffer = AudioBuf::<Stereo>::new(2);
/// let mut audio = buffer.audio();
/// let [left, _] = audio.channels_mut();
/// left.fill(1.0);
/// // At width 0 both channels carry the mid.
/// widen(&mut audio, 0.0);
/// assert_eq!(buffer.channels(), [[0.5, 0.5], [0.5, 0.5]]);
/// ```
#[doc(alias = "AudioBuffer")]
pub struct Audio<'a, L: Layout = AnyLayout> {
    /// The first `count` are the channels; the rest are empty.
    channels: [&'a mut [f32]; MAX_CHANNELS],
    count: usize,
    frames: usize,
    layout: PhantomData<L>,
}

impl<'a, L: Layout> Audio<'a, L> {
    /// Lends `channels`, each cut to its first `frames` samples, as audio in
    /// layout `L`: for an exporter, or for a test over buffers of its own.
    ///
    /// # Panics
    ///
    /// If there are more than [`MAX_CHANNELS`] channels, if one holds fewer
    /// than `frames` samples, or if `L` fixes its channels and there are
    /// another number of them.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_audio::Audio;
    ///
    /// let (mut left, mut right) = ([1.0_f32; 8], [1.0_f32; 8]);
    /// let mut channels = [&mut left[..], &mut right[..]];
    /// // The first 4 frames only.
    /// let mut audio: Audio<'_> = Audio::new(&mut channels, 4);
    /// audio.apply_gain(0.5);
    /// assert_eq!(left, [0.5, 0.5, 0.5, 0.5, 1.0, 1.0, 1.0, 1.0]);
    /// ```
    pub fn new(channels: &'a mut [&mut [f32]], frames: usize) -> Self {
        let count = channels.len();
        assert!(
            count <= MAX_CHANNELS,
            "audio holds at most {MAX_CHANNELS} channels, not {count}"
        );
        assert!(
            L::fits(count),
            "this layout fixes its channels, and {count} is not its count"
        );
        assert!(
            channels.iter().all(|channel| channel.len() >= frames),
            "a channel holds fewer than the {frames} frames asked for"
        );
        let channels = channels.iter_mut().map(|channel| {
            let channel: &'a mut [f32] = channel;
            channel.get_mut(..frames).unwrap_or_default()
        });
        Audio::over(channels, frames)
    }

    /// Lends `channels`, each `frames` long, of which there are at most
    /// [`MAX_CHANNELS`] and as many as `L` fixes.
    pub(crate) fn over(channels: impl Iterator<Item = &'a mut [f32]>, frames: usize) -> Self {
        let mut audio = Audio {
            channels: Default::default(),
            count: 0,
            frames,
            layout: PhantomData,
        };
        for (slot, channel) in audio.channels.iter_mut().zip(channels) {
            *slot = channel;
            audio.count = audio.count.saturating_add(1);
        }
        audio
    }

    /// How many frames it holds: its length in time.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Splits it at frame `at`: the first part holds every channel's frames
    /// before it, the second the rest, both in the same layout. Past its
    /// end, the second is empty. Nothing is copied.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_audio::{AudioBuf, layout::Stereo};
    ///
    /// let mut buffer = AudioBuf::<Stereo>::new(8);
    /// let (mut early, mut late) = buffer.audio().split_at(3);
    /// early.apply_gain(0.0);
    /// late.add_to_all(std::iter::repeat(1.0));
    /// assert_eq!((early.frames(), late.frames()), (3, 5));
    /// assert_eq!(buffer.channels()[0], [0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0]);
    /// ```
    pub fn split_at(self, at: usize) -> (Audio<'a, L>, Audio<'a, L>) {
        let at = at.min(self.frames);
        let rest = self.frames.saturating_sub(at);
        let mut first: [&'a mut [f32]; MAX_CHANNELS] = Default::default();
        let mut second: [&'a mut [f32]; MAX_CHANNELS] = Default::default();
        for ((channel, head), tail) in self.channels.into_iter().zip(&mut first).zip(&mut second) {
            let (before, after) = channel.split_at_mut(at.min(channel.len()));
            *head = before;
            *tail = after;
        }
        let part = |channels: [&'a mut [f32]; MAX_CHANNELS], frames| Audio {
            channels,
            count: self.count,
            frames,
            layout: PhantomData,
        };
        (part(first, at), part(second, rest))
    }

    /// Each frame in turn, one moment across every channel. In a
    /// [`Bus`](crate::layout::Bus) a frame is an array of the channels'
    /// samples, so `for [left, right] in audio.frames_mut()` destructures it;
    /// in [`AnyLayout`] it is a [`Frame`](crate::Frame). Either takes a mono
    /// voice's sample into every channel with
    /// [`AddSample::add`](crate::AddSample::add).
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_audio::AudioBuf;
    ///
    /// // A sine into every channel, as a mono voice plays in any layout.
    /// let mut buffer = AudioBuf::stereo(48);
    /// let mut phase = 0.0_f32;
    /// for frame in buffer.audio().frames_mut() {
    ///     frame.add((phase * std::f32::consts::TAU).sin());
    ///     phase = (phase + 1.0 / 48.0).fract();
    /// }
    /// assert!(buffer.audio().peak() > 0.99);
    /// ```
    pub fn frames_mut(&mut self) -> impl ExactSizeIterator<Item = L::Frame<'_>> {
        let (count, frames) = (self.count, self.frames);
        L::frames_of(self.lend(), count, frames)
    }

    /// Each channel, as a slice: an array in a [`Bus`](crate::layout::Bus),
    /// so it destructures, and an iterator in [`AnyLayout`].
    pub fn channels(&self) -> L::Channels<'_> {
        let mut lent: [&[f32]; MAX_CHANNELS] = Default::default();
        for (slot, channel) in lent.iter_mut().zip(&self.channels) {
            *slot = channel;
        }
        L::channels_of(lent, self.count)
    }

    /// Each channel, as a slice, so every slice method in std applies. In a
    /// [`Bus`](crate::layout::Bus) it is an array, so
    /// `let [left, right] = audio.channels_mut()` reaches each channel by its
    /// place, and a pattern that names one the bus does not have fails to
    /// compile. In [`AnyLayout`] it is an iterator.
    ///
    /// # Examples
    ///
    /// A gain on the left channel of a stereo block alone:
    ///
    /// ```
    /// use hapto_audio::AudioBuf;
    /// use hapto_audio::layout::Stereo;
    ///
    /// let mut buffer = AudioBuf::<Stereo>::new(4);
    /// let mut audio = buffer.audio();
    /// audio.add_to_all([1.0; 4]);
    /// let [left, _] = audio.channels_mut();
    /// for sample in left {
    ///     *sample *= 0.5;
    /// }
    /// assert_eq!(buffer.channels(), [[0.5; 4], [1.0; 4]]);
    /// ```
    ///
    /// A filter with one state per channel, zipped in the loop header, in any
    /// layout: `std::iter::zip` takes a `Bus`'s array as it is.
    ///
    /// ```
    /// use hapto_audio::AudioBuf;
    ///
    /// let mut buffer = AudioBuf::stereo(3);
    /// let mut states = [0.0_f32; 2];
    /// for (channel, state) in std::iter::zip(buffer.audio().channels_mut(), &mut states) {
    ///     for sample in channel {
    ///         *state += 1.0;
    ///         *sample = *state;
    ///     }
    /// }
    /// assert_eq!(states, [3.0, 3.0]);
    /// ```
    pub fn channels_mut(&mut self) -> L::ChannelsMut<'_> {
        let count = self.count;
        L::channels_mut_of(self.lend(), count)
    }

    /// The channel numbered `index`, from 0, or `None` past the last: for
    /// [`AnyLayout`], whose count is known only at run time. In a
    /// [`Bus`](crate::layout::Bus), destructure
    /// [`channels_mut`](Self::channels_mut) instead.
    pub fn channel_mut(&mut self, index: usize) -> Option<&mut [f32]> {
        self.channels_mut().into_iter().nth(index)
    }

    /// Multiplies every sample by `gain`.
    pub fn apply_gain(&mut self, gain: f32) {
        for channel in self.channels_mut() {
            for sample in channel {
                *sample *= gain;
            }
        }
    }

    /// Sets every sample to 0.
    pub fn clear(&mut self) {
        for channel in self.channels_mut() {
            channel.fill(0.0);
        }
    }

    /// Adds `samples`, one per frame, into every channel, stopping at the
    /// shorter of the two: for an author who prefers a chain to a loop. It
    /// walks a [`Frame`] at a time in every layout, a `Bus` too, so it costs
    /// what a loop over frames in `AnyLayout` does.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_audio::AudioBuf;
    ///
    /// let mut buffer = AudioBuf::stereo(4);
    /// buffer.audio().add_to_all((0..).map(|frame| frame as f32));
    /// assert!(buffer.channels().all(|channel| channel == [0.0, 1.0, 2.0, 3.0]));
    /// ```
    pub fn add_to_all(&mut self, samples: impl IntoIterator<Item = f32>) {
        let (count, frames) = (self.count, self.frames);
        let frames = AnyLayout::frames_of(self.lend(), count, frames);
        for (frame, sample) in frames.zip(samples) {
            frame.add(sample);
        }
    }

    /// The largest magnitude of any sample, 0 for silence. It is NaN if any
    /// sample is, so output a filter broke never reads as a level.
    pub fn peak(&self) -> f32 {
        self.channels()
            .into_iter()
            .flatten()
            .fold(0.0, |peak: f32, sample| {
                let magnitude = sample.abs();
                if magnitude > peak || magnitude.is_nan() {
                    magnitude
                } else {
                    peak
                }
            })
    }

    /// The same audio, lent again in [`AnyLayout`]: for code written to run
    /// in any layout, such as a voice, called from a processor in a typed
    /// one.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_audio::layout::Stereo;
    /// use hapto_audio::{Audio, AudioBuf};
    ///
    /// /// A mono tone into every channel, however many there are.
    /// fn tone(out: &mut Audio<'_>, level: f32) {
    ///     for frame in out.frames_mut() {
    ///         frame.add(level);
    ///     }
    /// }
    ///
    /// let mut buffer = AudioBuf::<Stereo>::new(4);
    /// let mut audio = buffer.audio();
    /// tone(&mut audio.as_any_layout(), 0.25);
    /// assert_eq!(audio.peak(), 0.25);
    /// ```
    pub fn as_any_layout(&mut self) -> Audio<'_, AnyLayout> {
        let (count, frames) = (self.count, self.frames);
        Audio::over(self.lend().into_iter().take(count), frames)
    }
}

impl<L: Layout> Audio<'_, L> {
    /// Every channel, lent again for as long as `self` is borrowed; the
    /// first `count` are the channels.
    fn lend(&mut self) -> [&mut [f32]; MAX_CHANNELS] {
        let mut lent: [&mut [f32]; MAX_CHANNELS] = Default::default();
        for (slot, channel) in lent.iter_mut().zip(self.channels.iter_mut()) {
            *slot = &mut **channel;
        }
        lent
    }
}

impl<L: Layout> fmt::Debug for Audio<'_, L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Audio")
            .field("channels", &self.count)
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}

/// The frames of audio in any layout, each a [`Frame`] made by splitting
/// the next sample off the front of each channel, so no two frames share a
/// sample.
pub(crate) struct AnyFrames<'a> {
    /// What is left of each channel; the first `count` are the channels.
    channels: [&'a mut [f32]; MAX_CHANNELS],
    count: usize,
    remaining: usize,
}

impl<'a> AnyFrames<'a> {
    /// The frames of the first `count` of `channels`, each `frames` long.
    pub(crate) fn new(
        channels: [&'a mut [f32]; MAX_CHANNELS],
        count: usize,
        frames: usize,
    ) -> Self {
        AnyFrames {
            channels,
            count,
            remaining: frames,
        }
    }
}

impl<'a> Iterator for AnyFrames<'a> {
    type Item = Frame<'a>;

    // Without it, the loop that walks the frames may call `next` out of
    // line, copying each frame out of the call.
    #[inline]
    fn next(&mut self) -> Option<Frame<'a>> {
        self.remaining = self.remaining.checked_sub(1)?;
        let samples = self
            .channels
            .iter_mut()
            .take(self.count)
            .filter_map(|channel| channel.split_off_first_mut());
        Some(Frame::of(samples))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl ExactSizeIterator for AnyFrames<'_> {}

/// The frames of a bus of `N` channels, each an array made by splitting the
/// next sample off the front of every channel, so no two frames share a
/// sample.
pub(crate) struct BusFrames<'a, const N: usize> {
    /// What is left of each channel.
    channels: [&'a mut [f32]; N],
    remaining: usize,
}

impl<'a, const N: usize> BusFrames<'a, N> {
    /// The frames of `channels`, each `frames` long.
    pub(crate) fn new(channels: [&'a mut [f32]; N], frames: usize) -> Self {
        BusFrames {
            channels,
            remaining: frames,
        }
    }
}

/// [`BusFrames`] as an iterator for each count of channels listed, with a
/// name for each channel. A frame takes a sample from every channel or is
/// not made, which a pattern over the whole array says with nothing to
/// unwrap, and stable Rust has no such pattern for a generic count.
macro_rules! bus_frames {
    ($($count:literal: [$($channel:ident)*])*) => {$(
        impl<'a> Iterator for BusFrames<'a, $count> {
            type Item = [&'a mut f32; $count];

            // Without it, a release build with no link-time optimisation
            // calls `next` out of line for a wide bus.
            #[inline]
            fn next(&mut self) -> Option<Self::Item> {
                self.remaining = self.remaining.checked_sub(1)?;
                let [$($channel),*] = &mut self.channels;
                Some([$($channel.split_off_first_mut()?),*])
            }

            fn size_hint(&self) -> (usize, Option<usize>) {
                (self.remaining, Some(self.remaining))
            }
        }

        impl ExactSizeIterator for BusFrames<'_, $count> {}
    )*};
}

bus_frames! {
    0: []
    1: [a]
    2: [a b]
    3: [a b c]
    4: [a b c d]
    5: [a b c d e]
    6: [a b c d e f]
    7: [a b c d e f g]
    8: [a b c d e f g h]
    9: [a b c d e f g h i]
    10: [a b c d e f g h i j]
    11: [a b c d e f g h i j k]
    12: [a b c d e f g h i j k l]
    13: [a b c d e f g h i j k l m]
    14: [a b c d e f g h i j k l m n]
    15: [a b c d e f g h i j k l m n o]
    16: [a b c d e f g h i j k l m n o p]
}
