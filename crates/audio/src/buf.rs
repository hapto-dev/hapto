//! Audio that owns its channels, which it lends as [`Audio`].

use std::fmt;
use std::marker::PhantomData;

use crate::Audio;
use crate::layout::{AnyLayout, Layout, MAX_CHANNELS, Width};

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
    /// Zeroed audio at `width`, `frames` long. It allocates.
    ///
    /// # Panics
    ///
    /// If `width` has more than [`MAX_CHANNELS`] channels, or if `L` fixes
    /// its channels and `width` is not one of its
    /// [widths](Layout::WIDTHS).
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_audio::AudioBuf;
    /// use hapto_audio::layout::{StereoOut, Width};
    ///
    /// // An instrument's audio: two outputs, no input.
    /// let buffer = AudioBuf::<StereoOut>::new(Width::new(0, 2), 512);
    /// assert_eq!((buffer.channels().len(), buffer.frames()), (2, 512));
    /// ```
    pub fn new(width: Width, frames: usize) -> Self {
        let channels = width.channels();
        assert!(
            channels <= MAX_CHANNELS,
            "audio holds at most {MAX_CHANNELS} channels, not {channels}"
        );
        assert!(
            L::WIDTHS.is_empty() || L::WIDTHS.contains(&width),
            "this layout runs at {:?}, not at {width:?}",
            L::WIDTHS
        );
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

    /// Each channel, as a slice.
    pub fn channels(&self) -> impl ExactSizeIterator<Item = &[f32]> + DoubleEndedIterator {
        self.channels.iter().map(Vec::as_slice)
    }
}

impl AudioBuf {
    /// Zeroed audio of one channel, `frames` long, in
    /// [`AnyLayout`]: what a voice is tested in. It allocates.
    pub fn mono(frames: usize) -> Self {
        AudioBuf::new(Width::MONO, frames)
    }

    /// Zeroed audio of two channels, `frames` long, in [`AnyLayout`]. It
    /// allocates.
    pub fn stereo(frames: usize) -> Self {
        AudioBuf::new(Width::STEREO, frames)
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
