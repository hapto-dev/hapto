//! The audio a voice renders into, with its note's place between the
//! speakers.

use std::fmt;

use hapto_audio::layout::{Layout, MAX_CHANNELS, StereoOut};
use hapto_audio::{AddSample, Audio};

use crate::values::Centred;

/// The audio a [`Voice`](super::Voice) renders into: the span's, in the
/// processor's own layout, other voices' sound in it already, and a gain for
/// each channel from its note's [`pan`](super::Expression::pan). Each of its
/// [frames](Self::frames_mut) adds a mono voice's sample at those gains, so
/// the voice is placed without a word about it. Its layout defaults as
/// [`Voice`](super::Voice)'s does.
///
/// The pan law is balanced, as JUCE's default is: at the centre both sides
/// are at full level, and a side fades as the note moves away from it. A
/// mono layout, and every channel past the first two, plays at full level.
/// A voice that places itself, a stereo sampler say, writes
/// [`audio_mut`](Self::audio_mut) instead.
///
/// # Examples
///
/// ```
/// use hapto_audio::AudioBuf;
/// use hapto_audio::layout::StereoOut;
/// use hapto_midi::{Centred, VoiceAudio};
///
/// // A voice panned halfway right.
/// let mut buffer = AudioBuf::<StereoOut>::new(2);
/// let mut out = VoiceAudio::new(buffer.audio(), Centred::new(0.5));
/// for frame in out.frames_mut() {
///     frame.add(1.0);
/// }
/// assert_eq!(buffer.channels(), [[0.5, 0.5], [1.0, 1.0]]);
/// ```
#[derive(Debug)]
pub struct VoiceAudio<'a, L: Layout = StereoOut> {
    audio: Audio<'a, L>,
    gains: [f32; MAX_CHANNELS],
}

impl<'a, L: Layout> VoiceAudio<'a, L> {
    /// `audio`, into which a voice at `pan` renders: what
    /// [`Poly`](super::Poly) hands each voice, and what a voice's own test
    /// makes.
    pub fn new(audio: Audio<'a, L>, pan: Centred) -> Self {
        let mut gains = [1.0; MAX_CHANNELS];
        if audio.channels().into_iter().count() >= 2
            && let [left, right, ..] = &mut gains
        {
            let pan = pan.get();
            *left = (1.0 - pan).min(1.0);
            *right = (1.0 + pan).min(1.0);
        }
        VoiceAudio { audio, gains }
    }

    /// How many frames it holds: the span's length.
    pub fn frames(&self) -> usize {
        self.audio.frames()
    }

    /// Each frame in turn, to add a mono voice's sample into at the note's
    /// place.
    pub fn frames_mut(&mut self) -> impl ExactSizeIterator<Item = VoiceFrame<'_, L>> {
        let gains = &self.gains;
        self.audio
            .frames_mut()
            .map(move |frame| VoiceFrame { frame, gains })
    }

    /// The gain of each channel, in order: where the note sits.
    pub fn gains(&self) -> &[f32] {
        let count = self.audio.channels().into_iter().count();
        self.gains.get(..count).unwrap_or_default()
    }

    /// The audio itself, which knows nothing of the note's place: for a
    /// voice that places itself.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_audio::AudioBuf;
    /// use hapto_audio::layout::StereoOut;
    /// use hapto_midi::{Centred, VoiceAudio};
    ///
    /// // A stereo voice, wide whatever the pan.
    /// let mut buffer = AudioBuf::<StereoOut>::new(1);
    /// let mut out = VoiceAudio::new(buffer.audio(), Centred::new(-1.0));
    /// for [left, right] in out.audio_mut().frames_mut() {
    ///     (*left, *right) = (0.25, -0.25);
    /// }
    /// assert_eq!(buffer.channels(), [[0.25], [-0.25]]);
    /// ```
    pub fn audio_mut(&mut self) -> &mut Audio<'a, L> {
        &mut self.audio
    }
}

/// One frame of a [`VoiceAudio`], which adds a mono voice's sample at its
/// note's place, or hands out the layout's own frame.
///
/// # Examples
///
/// ```
/// use hapto_audio::AudioBuf;
/// use hapto_audio::layout::StereoOut;
/// use hapto_midi::{Centred, VoiceAudio};
///
/// let mut buffer = AudioBuf::<StereoOut>::new(2);
/// {
///     let mut out = VoiceAudio::new(buffer.audio(), Centred::new(1.0));
///     let mut frames = out.frames_mut();
///     // The first frame placed, hard right; the second written as it is.
///     frames.next().expect("two frames").add(1.0);
///     let [left, right] = frames.next().expect("two frames").into_frame();
///     (*left, *right) = (0.5, 0.5);
/// }
/// assert_eq!(buffer.channels(), [[0.0, 0.5], [1.0, 0.5]]);
/// ```
pub struct VoiceFrame<'a, L: Layout = StereoOut> {
    frame: L::Frame<'a>,
    gains: &'a [f32; MAX_CHANNELS],
}

impl<'a, L: Layout> VoiceFrame<'a, L> {
    /// Adds `sample` into each channel at its gain: a mono voice, placed.
    pub fn add(self, sample: f32) {
        self.frame.add_scaled(sample, self.gains);
    }

    /// The layout's own frame, which knows nothing of the note's place:
    /// `[left, right]` in a stereo bus.
    pub fn into_frame(self) -> L::Frame<'a> {
        self.frame
    }
}

impl<L: Layout> fmt::Debug for VoiceFrame<'_, L> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VoiceFrame")
            .field("gains", self.gains)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use hapto_audio::AudioBuf;
    use hapto_audio::layout::{Bus, Mono, StereoOut};

    use super::VoiceAudio;
    use crate::Centred;

    fn gains_at(pan: f32) -> Vec<f32> {
        let mut buffer = AudioBuf::<StereoOut>::new(1);
        VoiceAudio::new(buffer.audio(), Centred::new(pan))
            .gains()
            .to_vec()
    }

    #[test]
    fn the_centre_is_full_level_on_both_sides_and_a_side_fades_away_from_it() {
        assert_eq!(gains_at(0.0), [1.0, 1.0]);
        assert_eq!(gains_at(-1.0), [1.0, 0.0]);
        assert_eq!(gains_at(1.0), [0.0, 1.0]);
        assert_eq!(gains_at(-0.25), [1.0, 0.75]);
    }

    #[test]
    fn mono_and_the_channels_past_two_play_at_full_level() {
        let mut mono = AudioBuf::<Mono>::new(1);
        assert_eq!(
            VoiceAudio::new(mono.audio(), Centred::new(1.0)).gains(),
            [1.0]
        );
        let mut quad = AudioBuf::<Bus<0, 4>>::new(1);
        assert_eq!(
            VoiceAudio::new(quad.audio(), Centred::new(1.0)).gains(),
            [0.0, 1.0, 1.0, 1.0]
        );
    }

    #[test]
    fn any_layout_places_a_voice_as_a_typed_one_does() {
        let mut buffer = AudioBuf::stereo(2);
        let mut out = VoiceAudio::new(buffer.audio(), Centred::new(-0.5));
        for frame in out.frames_mut() {
            frame.add(1.0);
        }
        let channels: Vec<&[f32]> = buffer.channels().collect();
        assert_eq!(channels, [[1.0, 1.0], [0.5, 0.5]]);
    }
}
