//! The synth played with no host: one phrase in each form a host can send
//! it, at any block size, and the rule for a key struck twice.

use hapto_example_synth::Synth;
use hapto_host::midi::Key;
use hapto_host::play::Play;
use hapto_ump::{Attribute, ChannelMessage, scale};

const RATE: f64 = 48_000.0;
/// A4's number, for MIDI written as bytes and as MIDI 2.0's messages.
const A4: u8 = Key::A4.wire();

#[test]
fn a_key_sounds_until_its_release_has_faded() {
    let mut play = Play::<Synth>::new(RATE);
    play.midi1(0, &[0x90, A4, 127]);
    play.midi1(4_800, &[0x80, A4, 0]);
    let sound = play.render(19_200);
    assert!(sound.peak(0..4_800) > 0.1);
    assert!(
        sound.is_silent(16_000..19_200),
        "faded 0.2 s after the release"
    );
}

#[test]
fn the_sound_is_the_same_whatever_the_block_size() {
    let render = |blocks: &[usize]| {
        let mut play = Play::<Synth>::new(RATE).with_blocks(blocks);
        in_midi1(&mut play);
        play.render(19_200)
    };
    let whole = render(&[19_200]);
    assert_eq!(render(&[64]), whole);
    assert_eq!(render(&[1, 999, 13, 1024]), whole);
}

#[test]
fn a_phrase_sounds_the_same_however_the_host_sends_it() {
    let render = |send: fn(&mut Play<Synth>)| {
        let mut play = Play::<Synth>::new(RATE);
        send(&mut play);
        play.render(19_200)
    };
    let midi1 = render(in_midi1);
    assert!(midi1.peak(0..4_800) > 0.1);
    assert_eq!(render(in_midi2), midi1);
    assert_eq!(render(in_midi2_per_note), midi1);
    assert_eq!(render(with_note_ids), midi1);
}

#[test]
fn a_note_off_ends_every_note_on_its_key() {
    let mut play = Play::<Synth>::new(RATE);
    play.midi1(0, &[0x90, A4, 127]);
    play.midi1(2_400, &[0x90, A4, 127]);
    play.midi1(4_800, &[0x80, A4, 0]);
    let sound = play.render(19_200);
    assert!(sound.is_silent(16_000..19_200));
}

#[test]
fn notes_a_host_names_by_id_stay_apart_on_one_key() {
    let mut play = Play::<Synth>::new(RATE);
    let first = play.note_on(0, Key::A4, 1.0);
    play.note_on(2_400, Key::A4, 1.0);
    play.note_off(4_800, first);
    let sound = play.render(19_200);
    assert!(sound.peak(16_000..19_200) > 0.1, "the second still sounds");
}

#[test]
fn the_sustain_pedal_holds_a_released_key_until_it_comes_up() {
    let mut play = Play::<Synth>::new(RATE);
    play.midi1(0, &[0xB0, 64, 127, 0x90, A4, 127]);
    play.midi1(2_400, &[0x80, A4, 0]);
    play.midi1(14_400, &[0xB0, 64, 0]);
    let sound = play.render(33_600);
    assert!(sound.peak(12_000..14_400) > 0.1, "held by the pedal");
    assert!(
        sound.is_silent(28_800..33_600),
        "faded once the pedal came up"
    );
}

// One phrase: A4 at full velocity, bent up a whole step at 50 ms, pressed
// fully at 60 ms, released at 100 ms. Each value is a protocol's maximum, so
// every form says exactly the same thing.

/// In MIDI 1.0, on the channel.
fn in_midi1(play: &mut Play<Synth>) {
    play.midi1(0, &[0x90, A4, 127]);
    play.midi1(2_400, &[0xE0, 0x7F, 0x7F]);
    play.midi1(2_880, &[0xD0, 127]);
    play.midi1(4_800, &[0x80, A4, 0]);
}

/// In MIDI 2.0, on the channel.
fn in_midi2(play: &mut Play<Synth>) {
    play.midi2(0, note_on());
    play.midi2(2_400, ChannelMessage::PitchBend { value: u32::MAX });
    play.midi2(2_880, ChannelMessage::ChannelPressure { value: u32::MAX });
    play.midi2(4_800, note_off());
}

/// In MIDI 2.0, on the note alone.
fn in_midi2_per_note(play: &mut Play<Synth>) {
    play.midi2(0, note_on());
    // Registered per-note controller 3: an absolute pitch, a whole step up.
    play.midi2(
        2_400,
        ChannelMessage::RegisteredPerNoteController {
            note: A4,
            index: 3,
            value: scale::pitch_7_25(71.0),
        },
    );
    play.midi2(
        2_880,
        ChannelMessage::PolyPressure {
            note: A4,
            value: u32::MAX,
        },
    );
    play.midi2(4_800, note_off());
}

/// As a host that names notes by id sends it, as VST3 does.
fn with_note_ids(play: &mut Play<Synth>) {
    let note = play.note_on(0, Key::A4, 1.0);
    play.tuning(2_400, note, 2.0);
    play.pressure(2_880, note, 1.0);
    play.note_off(4_800, note);
}

fn note_on() -> ChannelMessage {
    ChannelMessage::NoteOn {
        note: A4,
        velocity: u16::MAX,
        attribute: Attribute::NONE,
    }
}

fn note_off() -> ChannelMessage {
    ChannelMessage::NoteOff {
        note: A4,
        velocity: 0,
        attribute: Attribute::NONE,
    }
}
