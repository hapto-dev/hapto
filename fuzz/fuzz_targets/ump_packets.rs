//! The UMP packet stream a host hands a plugin: split into packets, and
//! decode each as a MIDI 2.0 or a MIDI 1.0 channel voice message. Nothing
//! panics, a decoded message encodes to a packet that decodes to itself,
//! and the packets consume the stream exactly.
#![no_main]

use libfuzzer_sys::fuzz_target;
use hapto_ump::{ChannelVoice, Midi1Voice, Packets, packet_words};

fuzz_target!(|data: &[u8]| {
    let words: Vec<u32> = data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&chunk| u32::from_le_bytes(chunk))
        .collect();
    let offset = u32::from(data.first().copied().unwrap_or(0));
    let mut consumed = 0;
    for packet in Packets::new(&words, offset) {
        match packet {
            Ok(event) => {
                assert_eq!(event.offset, offset);
                let size = packet_words((event.words[0] >> 28) as u8).expect("a four-bit type");
                consumed += size;
                if let Some(voice) = ChannelVoice::decode(event.words) {
                    assert_eq!(ChannelVoice::decode(voice.words()), Some(voice));
                }
                if let Some(voice) = Midi1Voice::decode(event.words) {
                    assert_eq!(Midi1Voice::decode(voice.words()), Some(voice));
                }
            }
            Err(_) => consumed = words.len(),
        }
    }
    assert_eq!(consumed, words.len());
});
