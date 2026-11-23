//! A block's audio, as a processor sees it.
//!
//! A processor names its main bus's channels as a [layout](layout::Layout)
//! type, and its block lends it [`Audio`]: frames and channels to loop over,
//! with no index and no frame count to carry. In a [`Bus`](layout::Bus) the
//! channels are known at compile time, so frames and channels are arrays: a
//! stereo frame destructures as `[left, right]`, and
//! `let [left, right] = audio.channels_mut()` reaches each channel by its
//! place. [`AnyLayout`](layout::AnyLayout) counts them at run time.
//! [`AudioBuf`] owns audio of its own, for tests and scratch.
//!
//! The crate is in the core tier and depends on nothing, so code that writes
//! audio needs no host. Every call but those that make an `AudioBuf` is fit
//! for the audio thread: none allocates, and only [`Audio::new`], for a
//! caller's mistake, panics.
//!
//! # Examples
//!
//! A gain over a stereo block, and a mono tone that plays in any layout:
//!
//! ```
//! use hapto_audio::{Audio, AudioBuf};
//! use hapto_audio::layout::Stereo;
//!
//! fn gain(audio: &mut Audio<'_, Stereo>, gain: f32) {
//!     for [left, right] in audio.frames_mut() {
//!         *left *= gain;
//!         *right *= gain;
//!     }
//! }
//!
//! fn tone(out: &mut Audio<'_>, level: f32) {
//!     for frame in out.frames_mut() {
//!         // Into every channel, however many there are.
//!         frame.add(level);
//!     }
//! }
//!
//! let mut stereo = AudioBuf::<Stereo>::new(64);
//! stereo.audio().add_to_all([1.0; 64]);
//! gain(&mut stereo.audio(), 0.5);
//! assert_eq!(stereo.audio().peak(), 0.5);
//!
//! let mut voice = AudioBuf::mono(64);
//! tone(&mut voice.audio(), 0.25);
//! assert_eq!(voice.audio().peak(), 0.25);
//! ```

// Audio-path code: a panic here ends a host, so indexing, unwrap, expect,
// panic and unchecked arithmetic are refused.
#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects
    )
)]

mod audio;
mod buf;
mod frame;
pub mod layout;

pub use audio::Audio;
pub use buf::AudioBuf;
pub use frame::Frame;
