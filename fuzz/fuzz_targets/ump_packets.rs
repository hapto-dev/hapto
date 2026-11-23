//! Fuzzes the UMP stream that a host passes a plugin. It splits the stream
//! into packets, decodes each as a MIDI 2.0 or MIDI 1.0 channel voice message,
//! and reads each into messages as a plugin does. It checks that:
//!
//! - nothing panics
//! - a decoded message round-trips through its packet
//! - a decoder small enough to fill stays within its tables
//! - the packets use up the stream exactly
#![no_main]

use libfuzzer_sys::fuzz_target;
use hapto_midi::Decoder;
use hapto_ump::{ChannelVoice, Midi1Voice, Packets};

fuzz_target!(|data: &[u8]| {
    let words: Vec<u32> = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&chunk| u32::from_le_bytes(chunk))
        .collect();
    let offset = u32::from(data.first().copied().unwrap_or(0));
    let mut consumed = 0;
    let mut decoder = Decoder::with_room(4, 16);
    for packet in Packets::new(&words, offset) {
        match packet {
            Ok(event) => {
                assert_eq!(event.offset, offset);
                let size = event.message_type().words();
                consumed += size;
                if let Some(voice) = ChannelVoice::decode(event.words) {
                    assert_eq!(ChannelVoice::decode(voice.words()), Some(voice));
                }
                if let Some(voice) = Midi1Voice::decode(event.words) {
                    assert_eq!(Midi1Voice::decode(voice.words()), Some(voice));
                }
                for message in decoder.decode(&event) {
                    let _ = message.to_string();
                }
                assert!(decoder.sounding() <= 4);
            }
            Err(_) => consumed = words.len(),
        }
    }
    assert_eq!(consumed, words.len());
});
