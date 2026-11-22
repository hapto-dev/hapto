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
/// Open: whether walking frames vectorises worse than walking channels.
/// Hapto's instruction-count benchmarks are to measure the two before the
/// effect examples settle on one.
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
