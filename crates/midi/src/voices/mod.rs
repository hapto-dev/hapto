//! An instrument's voices. [`Poly`] gives each note a [`Voice`], computes
//! what it plays at as an [`Expression`], and gives it its audio as a
//! [`VoiceAudio`].

mod audio;
mod expression;
mod mpe;
mod poly;
mod state;
#[cfg(test)]
mod tests;
mod tuning;

use hapto_audio::layout::{Layout, StereoOut};

pub use audio::{VoiceAudio, VoiceFrame};
pub use expression::Expression;
pub use poly::{Poly, ToPlay};
pub use tuning::Tuning;

use crate::message::{NoteOff, NoteOn};

/// A plugin's voice, which renders the notes that [`Poly`] gives it into
/// audio in layout `L`.
///
/// `L` is [`StereoOut`], as in most instruments, unless the voice names
/// another, so a stereo voice is `impl Voice for Sine` and a mono one `impl
/// Voice<Mono> for Sine`. [`Poly`] and [`VoiceAudio`] take the same default.
/// A voice that renders the same in any layout is written once, as
/// `impl<L: Layout> Voice<L> for Sine`. One written for a layout reaches its
/// channels by name, such as `[left, right]` in a stereo bus.
///
/// # Examples
///
/// ```
/// use hapto_audio::AudioBuf;
/// use hapto_audio::layout::StereoOut;
/// use hapto_midi::{Centred, Expression, Key, NoteOff, NoteOn, Voice, VoiceAudio};
///
/// /// A decaying click.
/// #[derive(Default)]
/// struct Click {
///     level: f32,
/// }
///
/// impl Voice for Click {
///     fn start(&mut self, note: &NoteOn) {
///         self.level = note.velocity;
///     }
///
///     fn release(&mut self, _: &NoteOff) {}
///
///     fn render(&mut self, _: &Expression<'_>, out: &mut VoiceAudio<'_>) {
///         for frame in out.frames_mut() {
///             frame.add(self.level);
///             // Halving, and silent once below -80 dB.
///             self.level = if self.level < 1e-4 { 0.0 } else { 0.5 * self.level };
///         }
///     }
///
///     fn is_done(&self) -> bool {
///         self.level == 0.0
///     }
/// }
///
/// // A voice's test, panned hard left, needs no `Poly`.
/// let mut click = Click::default();
/// let mut buffer = AudioBuf::<StereoOut>::new(3);
/// let middle_c = Key::C4;
/// click.start(&NoteOn::new(middle_c, 1.0));
/// let mut out = VoiceAudio::new(buffer.audio(), Centred::new(-1.0));
/// click.render(&Expression::at(middle_c), &mut out);
/// assert_eq!(buffer.channels(), [[1.0, 0.5, 0.25], [0.0, 0.0, 0.0]]);
/// ```
pub trait Voice<L: Layout = StereoOut> {
    /// Starts a note, on a voice that was idle or that `Poly` took from
    /// another note.
    fn start(&mut self, note: &NoteOn);

    /// Releases the note, which neither its key nor a pedal holds any more.
    fn release(&mut self, note: &NoteOff);

    /// Adds the voice's next `out.frames()` frames into `out`, which holds
    /// the other voices' too.
    ///
    /// The audio is cut at each message, so `expression` holds the note's
    /// values until the next.
    fn render(&mut self, expression: &Expression<'_>, out: &mut VoiceAudio<'_, L>);

    /// Returns whether the voice has gone quiet, so `Poly` can free it.
    ///
    /// Once done, a voice adds nothing more, because `Poly` frees it after
    /// the span it rendered. A voice that kept sounding would sound different
    /// as the host's block size changes.
    fn is_done(&self) -> bool;
}
