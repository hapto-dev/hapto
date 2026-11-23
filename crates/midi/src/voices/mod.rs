//! An instrument's voices: [`Poly`] gives each note a [`Voice`], works out
//! what it plays at, its [`Expression`], and hands it its audio as a
//! [`VoiceAudio`].

mod audio;
mod expression;
mod poly;
mod state;
#[cfg(test)]
mod tests;
mod tuning;

use hapto_audio::layout::Layout;

pub use audio::{VoiceAudio, VoiceFrame};
pub use expression::Expression;
pub use poly::{Poly, ToPlay};
pub use tuning::Tuning;

use crate::message::{NoteOff, NoteOn};

/// One of a plugin's voices, which [`Poly`] gives notes to, rendering into
/// audio in layout `L`.
///
/// A voice that renders the same in any layout is written once for all:
/// `impl<L: Layout> Voice<L> for Sine`. One written for a layout reaches its
/// channels by name, `[left, right]` in a stereo bus.
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
/// impl Voice<StereoOut> for Click {
///     fn start(&mut self, note: &NoteOn) {
///         self.level = note.velocity;
///     }
///
///     fn release(&mut self, _: &NoteOff) {}
///
///     fn render(&mut self, _: &Expression<'_>, out: &mut VoiceAudio<'_, StereoOut>) {
///         for frame in out.frames_mut() {
///             frame.add(self.level);
///             self.level *= 0.5;
///         }
///     }
///
///     fn is_done(&self) -> bool {
///         self.level < 1e-4
///     }
/// }
///
/// // A voice's test needs no `Poly`: here, panned hard left.
/// let mut click = Click::default();
/// let mut buffer = AudioBuf::<StereoOut>::new(3);
/// click.start(&NoteOn::new(Key::C4, 1.0));
/// let mut out = VoiceAudio::new(buffer.audio(), Centred::new(-1.0));
/// click.render(&Expression::at(Key::C4), &mut out);
/// assert_eq!(buffer.channels(), [[1.0, 0.5, 0.25], [0.0, 0.0, 0.0]]);
/// ```
pub trait Voice<L: Layout> {
    /// Starts a note, on a voice that was idle or that `Poly` took from
    /// another note.
    fn start(&mut self, note: &NoteOn);

    /// Nothing holds the note any more: neither its key nor a pedal.
    fn release(&mut self, note: &NoteOff);

    /// Adds the voice's next `out.frames()` frames into `out`, which holds
    /// the other voices' too. The audio comes cut at each message, and
    /// `expression` holds the note's values until the next.
    fn render(&mut self, expression: &Expression<'_>, out: &mut VoiceAudio<'_, L>);

    /// Whether the voice has gone quiet, so `Poly` may free it.
    fn is_done(&self) -> bool;
}
