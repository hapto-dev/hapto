//! Decodes a block's MIDI packets into [`Message`]s in musicians' terms.
//!
//! MIDI reaches a plugin in one form, whatever its host sent, because MIDI
//! 1.0 is upgraded to MIDI 2.0 on the way in. [`Decoder`] reads it into
//! messages with MIDI 2.0's meanings:
//!
//! - A value from 0 to 1 is an `f32`.
//! - A value that rests at a centre, such as a bend, is [`Centred`], from -1
//!   to 1 with 0 at rest.
//! - Groups, channels, keys and programs have their own types, stored as
//!   MIDI sends them and shown as musicians count, such as channels 1 to 16,
//!   middle C as C4 and programs 1 to 128.
//! - A [`NoteId`] names a note from its note-on to its note-off, and a
//!   per-note message names its note with a [`NoteKey`].
//!
//! A block's audio cut at each message is its [`Spans`]. An instrument's
//! notes play on its [`Voice`]s, which [`Poly`] gives them.
//!
//! The crate is in the core tier, on `hapto-ump`, `hapto-audio` and
//! `hapto-params`, and depends on nothing else. Decoding, reading a block,
//! cutting it into spans, sending and playing voices are fit for the audio
//! thread: they allocate nothing and never panic.
//!
//! # Examples
//!
//! A note's life, decoded:
//!
//! ```
//! use hapto_midi::{Decoder, Key, Message};
//! use hapto_ump::Event;
//!
//! let mut decoder = Decoder::new();
//! let on = Event::new(0, [0x4090_3C00, 0xFFFF_0000, 0, 0]);
//! let off = Event::new(32, [0x4080_3C00, 0x8000_0000, 0, 0]);
//!
//! let Some(Message::NoteOn(note)) = decoder.decode(&on).next() else {
//!     panic!("a note-on");
//! };
//! let middle_c = Key::C4;
//! assert_eq!((note.key, note.velocity), (middle_c, 1.0));
//! assert_eq!(note.channel.to_string(), "1");
//!
//! // Its note-off names the same note.
//! let Some(Message::NoteOff(ended)) = decoder.decode(&off).next() else {
//!     panic!("a note-off");
//! };
//! assert_eq!(ended.id, note.id);
//! ```

// The nightly real-time sanitizer lane marks the calls that the audio thread
// makes.
#![cfg_attr(hapto_rtsan, feature(sanitize))]
// A panic on the audio path ends the host, so the crate denies indexing,
// unwrap, expect, panic and unchecked arithmetic outside its unit tests. A
// site that is provably safe gives its reason in an `#[expect]`.
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

mod channels;
pub mod controller;
mod decoder;
mod encode;
mod message;
mod midi;
mod spans;
mod values;
mod voices;

pub use channels::PerChannel;
pub use controller::{Controller, ControllerValue};
pub use decoder::{Decoder, Messages, NOTES, SYSEX_BYTES};
pub use message::{Message, NoteKey, NoteOff, NoteOn};
pub use midi::{BLOCK_SYSEX_BYTES, Iter, MESSAGES, Midi, MidiList, MidiOut, Packets, packets_of};
pub use spans::{Span, Spans};
pub use values::{Centred, Channel, Group, Key, NoteId, Pedal, Pitch, Program};
pub use voices::{Expression, Poly, ToPlay, Tuning, Voice, VoiceAudio, VoiceFrame};
