//! Audio that owns its channels, which it lends as [`Audio`].

use std::fmt;
use std::marker::PhantomData;

use crate::Audio;
use crate::layout::{AnyLayout, Bus, Layout, MAX_CHANNELS, Width};

/// Channels of its own, zeroed when made, which it lends as [`Audio`]: for
/// testing a voice or a processor on its own, or for a plugin's scratch
/// audio, made at activation.
///
/// Its name follows the standard library's: as a `PathBuf` owns what a
/// `Path` borrows, an `AudioBuf` owns what an [`Audio`] borrows.
///
/// # Examples
///
/// ```
/// use hapto_audio::{Audio, AudioBuf};
///
/// fn half(audio: &mut Audio<'_>) {
///     audio.apply_gain(0.5);
/// }
///
/// let mut buffer = AudioBuf::stereo(4);
/// buffer.audio().add_to_all([1.0; 4]);
/// half(&mut buffer.audio());
/// assert_eq!(buffer.audio().peak(), 0.5);
/// ```
#[derive(Clone)]
pub struct AudioBuf<L: Layout = AnyLayout> {
    channels: Vec<Vec<f32>>,
    frames: usize,
    layout: PhantomData<L>,
}

impl<L: Layout> AudioBuf<L> {
    /// Zeroed audio of `channels` channels, `frames` long, which the caller
    /// has checked `L` takes.
    fn zeroed(channels: usize, frames: usize) -> Self {
        AudioBuf {
            channels: vec![vec![0.0; frames]; channels],
            frames,
            layout: PhantomData,
        }
    }

    /// Lends every channel, as a block lends the host's.
    pub fn audio(&mut self) -> Audio<'_, L> {
        Audio::over(self.channels.iter_mut().map(Vec::as_mut_slice), self.frames)
    }

    /// How many frames it holds.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Each channel, as a slice: an array in a [`Bus`], so it destructures,
    /// and an iterator in [`AnyLayout`].
    pub fn channels(&self) -> L::Channels<'_> {
        let mut lent: [&[f32]; MAX_CHANNELS] = Default::default();
        for (slot, channel) in lent.iter_mut().zip(&self.channels) {
            *slot = channel;
        }
        L::channels_of(lent, self.channels.len())
    }
}

impl<const IN: usize, const OUT: usize> AudioBuf<Bus<IN, OUT>>
where
    Bus<IN, OUT>: Layout,
{
    /// Zeroed audio at the bus's own width, `frames` long. It allocates.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_audio::AudioBuf;
    /// use hapto_audio::layout::StereoOut;
    ///
    /// // An instrument's audio: two outputs, no input.
    /// let buffer = AudioBuf::<StereoOut>::new(512);
    /// let [left, right] = buffer.channels();
    /// assert_eq!((left.len(), right.len()), (512, 512));
    /// ```
    pub fn new(frames: usize) -> Self {
        AudioBuf::zeroed(Width::new(IN, OUT).channels(), frames)
    }
}

impl AudioBuf {
    /// Zeroed audio at `width`, `frames` long, in [`AnyLayout`]. It
    /// allocates.
    ///
    /// # Panics
    ///
    /// If `width` has more than [`MAX_CHANNELS`] channels.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_audio::AudioBuf;
    /// use hapto_audio::layout::{AnyLayout, Width};
    ///
    /// // A 5.1 bus, its count known only at run time.
    /// let buffer = AudioBuf::<AnyLayout>::new(Width::new(6, 6), 512);
    /// assert_eq!(buffer.channels().len(), 6);
    /// ```
    pub fn new(width: Width, frames: usize) -> Self {
        let channels = width.channels();
        assert!(
            channels <= MAX_CHANNELS,
            "audio holds at most {MAX_CHANNELS} channels, not {channels}"
        );
        AudioBuf::zeroed(channels, frames)
    }

    /// Zeroed audio of one channel, `frames` long, in
    /// [`AnyLayout`]: what a voice is tested in. It allocates.
    pub fn mono(frames: usize) -> Self {
        AudioBuf::zeroed(1, frames)
    }

    /// Zeroed audio of two channels, `frames` long, in [`AnyLayout`]. It
    /// allocates.
    pub fn stereo(frames: usize) -> Self {
        AudioBuf::zeroed(2, frames)
    }
}

impl<L: Layout> fmt::Debug for AudioBuf<L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AudioBuf")
            .field("channels", &self.channels.len())
            .field("frames", &self.frames)
            .finish_non_exhaustive()
    }
}
