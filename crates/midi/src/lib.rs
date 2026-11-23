//! MIDI in musicians' terms: what a block's packets say, decoded into
//! [`Message`]s.
//!
//! MIDI reaches a plugin in one form whatever its host sent: MIDI 1.0 is
//! upgraded to MIDI 2.0 on the way in, and [`Decoder`] reads MIDI 2.0 into
//! messages in MIDI 2.0's meanings. A value from 0 to 1 is an `f32`, and one
//! that rests at a centre, a bend or a brightness, is [`Centred`]: -1 to 1,
//! 0 at rest. Groups, channels, keys and programs are types of their own,
//! stored as MIDI sends them and shown as musicians count them: channel 1 to
//! 16, middle C as C4, program 1 to 128. A note is named by its [`NoteId`]
//! from its note-on to its note-off, and a note's own message by the
//! [`NoteKey`] it names. [`controller`] names MIDI's controllers.
//!
//! A block's MIDI is a [`Midi`]: its messages, each at its frame, and the
//! packets they came in, read from a [`MidiList`] the engine decodes each
//! block into once. What a block sends goes through its [`MidiOut`].
//!
//! The crate is in the core tier, on `hapto-ump`, and depends on nothing
//! else. Decoding, reading a block and sending are fit for the audio thread:
//! they allocate nothing and never panic.
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

// The nightly real-time sanitizer lane marks the calls the audio thread makes.
#![cfg_attr(hapto_rtsan, feature(sanitize))]

pub mod controller;
mod decoder;
mod encode;
mod message;
mod midi;
mod values;

pub use controller::{Controller, ControllerValue};
pub use decoder::{Decoder, Messages, NOTES, SYSEX_BYTES};
pub use message::{Message, NoteKey, NoteOff, NoteOn};
pub use midi::{BLOCK_SYSEX_BYTES, Iter, MESSAGES, Midi, MidiList, MidiOut, Packets};
pub use values::{Centred, Channel, Group, Key, NoteId, Pedal, Pitch, Program};
