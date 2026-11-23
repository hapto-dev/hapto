//! One frame of audio whose channels are counted at run time.

use std::fmt;

use crate::layout::MAX_CHANNELS;

/// One frame of audio whose channels are counted at run time: one sample of
/// each channel, at one moment, as
/// [`Audio::frames_mut`](crate::Audio::frames_mut) hands it out in
/// [`AnyLayout`](crate::layout::AnyLayout).
///
/// It holds up to [`MAX_CHANNELS`] samples inline, so making one allocates
/// nothing.
///
/// Open: a `Frame` is built and walked a slot at a time, so a loop over
/// frames in `AnyLayout` costs far more than one over channels. A loop that
/// carries a plugin's load walks [`channels_mut`](crate::Audio::channels_mut)
/// or the frames of a [`Bus`](crate::layout::Bus); on a bus wider than about
/// eight channels, one that does the same to every sample walks channels.
///
/// # Examples
///
/// ```
/// use hapto_audio::AudioBuf;
///
/// let mut buffer = AudioBuf::stereo(2);
/// for mut frame in buffer.audio().frames_mut() {
///     // Left at full level, right at half.
///     for (sample, gain) in frame.samples_mut().zip([1.0, 0.5]) {
///         *sample += gain;
///     }
/// }
/// let channels: Vec<&[f32]> = buffer.channels().collect();
/// assert_eq!(channels, [[1.0, 1.0], [0.5, 0.5]]);
/// ```
pub struct Frame<'a> {
    samples: [Option<&'a mut f32>; MAX_CHANNELS],
}

impl<'a> Frame<'a> {
    /// The frame of `samples`, one per channel, of which it keeps the first
    /// [`MAX_CHANNELS`].
    pub(crate) fn of(samples: impl Iterator<Item = &'a mut f32>) -> Self {
        let mut frame = Frame {
            samples: Default::default(),
        };
        for (slot, sample) in frame.samples.iter_mut().zip(samples) {
            *slot = Some(sample);
        }
        frame
    }

    /// Adds `sample` into each of the frame's channels: a mono source's way
    /// to play in any layout.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_audio::AudioBuf;
    ///
    /// let mut buffer = AudioBuf::stereo(3);
    /// for frame in buffer.audio().frames_mut() {
    ///     frame.add(0.25);
    /// }
    /// assert!(buffer.channels().all(|channel| channel == [0.25; 3]));
    /// ```
    pub fn add(self, sample: f32) {
        for channel in self.samples.into_iter().flatten() {
            *channel += sample;
        }
    }

    /// The frame's samples, one per channel, in order.
    pub fn samples_mut(&mut self) -> impl Iterator<Item = &mut f32> {
        self.samples
            .iter_mut()
            .flatten()
            .map(|sample| &mut **sample)
    }

    /// How many channels the frame has.
    pub fn len(&self) -> usize {
        self.samples.iter().flatten().count()
    }

    /// Whether the frame has no channels: audio of no channels has frames
    /// all the same, to count its time.
    pub fn is_empty(&self) -> bool {
        self.samples.first().is_none_or(Option::is_none)
    }
}

impl fmt::Debug for Frame<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.samples.iter().flatten())
            .finish()
    }
}

/// A frame that adds one sample to each of its channels, as a mono voice
/// plays.
///
/// Every layout's frame, a [`Bus`](crate::layout::Bus)'s array or a
/// [`Frame`], implements it, so code over any
/// [`Layout`](crate::layout::Layout) adds a voice the same way.
///
/// # Examples
///
/// ```
/// use hapto_audio::layout::Stereo;
/// use hapto_audio::{AddSample, AudioBuf};
///
/// let mut buffer = AudioBuf::<Stereo>::new(2);
/// for frame in buffer.audio().frames_mut() {
///     frame.add(0.25);
/// }
/// assert_eq!(buffer.channels(), [[0.25, 0.25], [0.25, 0.25]]);
/// ```
pub trait AddSample {
    /// Adds `sample` to each of the frame's channels.
    fn add(self, sample: f32);

    /// Adds `sample` into each of the frame's channels at that channel's
    /// gain, the first gain for the first channel: a mono voice placed
    /// between the speakers. A channel past the last gain gets none of it.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_audio::layout::Stereo;
    /// use hapto_audio::{AddSample, AudioBuf};
    ///
    /// // Hard left.
    /// let mut buffer = AudioBuf::<Stereo>::new(2);
    /// for frame in buffer.audio().frames_mut() {
    ///     frame.add_scaled(0.5, &[1.0, 0.0]);
    /// }
    /// assert_eq!(buffer.channels(), [[0.5, 0.5], [0.0, 0.0]]);
    /// ```
    fn add_scaled(self, sample: f32, gains: &[f32]);
}

impl<const N: usize> AddSample for [&mut f32; N] {
    fn add(self, sample: f32) {
        for channel in self {
            *channel += sample;
        }
    }

    fn add_scaled(self, sample: f32, gains: &[f32]) {
        for (channel, gain) in self.into_iter().zip(gains) {
            *channel += sample * gain;
        }
    }
}

impl AddSample for Frame<'_> {
    fn add(self, sample: f32) {
        Frame::add(self, sample);
    }

    fn add_scaled(self, sample: f32, gains: &[f32]) {
        for (channel, gain) in self.samples.into_iter().flatten().zip(gains) {
            *channel += sample * gain;
        }
    }
}
