use hapto_audio::AudioBuf;
use hapto_audio::Status;
use hapto_audio::layout::Mono;
use hapto_ump::{Event, EventList};

use super::{Expression, Poly, Tuning, Voice, VoiceAudio};
use crate::controller::{self, number};
use crate::{
    Centred, Channel, Group, Key, Message, MidiList, NoteId, NoteKey, NoteOff, NoteOn, Pitch,
};

/// A voice that keeps what it was told and what it last rendered at.
#[derive(Debug, Default)]
struct Probe {
    note: Option<NoteOn>,
    released: Option<NoteOff>,
    starts: usize,
    pitch: f32,
    pressure: f32,
    timbre: f32,
    damper: f32,
    volume: f32,
    done: bool,
}

impl Voice<Mono> for Probe {
    fn start(&mut self, note: &NoteOn) {
        *self = Probe {
            note: Some(*note),
            starts: self.starts + 1,
            ..Probe::default()
        };
    }

    fn release(&mut self, note: &NoteOff) {
        assert!(self.released.is_none(), "released twice");
        self.released = Some(*note);
    }

    fn render(&mut self, expression: &Expression<'_>, out: &mut VoiceAudio<'_, Mono>) {
        self.pitch = expression.pitch.semitones();
        self.pressure = expression.pressure;
        self.timbre = expression.timbre.get();
        self.damper = expression.damper;
        self.volume = expression.get(controller::VOLUME);
        for frame in out.frames_mut() {
            frame.add(1.0);
        }
    }

    fn is_done(&self) -> bool {
        self.done
    }
}

fn poly(count: usize) -> Poly<Probe, Mono> {
    Poly::new(count, Probe::default)
}

fn on(key: u8, id: u32) -> Message<'static> {
    Message::NoteOn(NoteOn::new(Key::new(key), 1.0).with_id(NoteId::own(id)))
}

fn off(key: u8, id: u32) -> Message<'static> {
    let note = NoteOn::new(Key::new(key), 1.0).with_id(NoteId::own(id));
    Message::NoteOff(note.off(0.5))
}

fn control(controller: u8, value: f32) -> Message<'static> {
    Message::Control {
        group: Group::FIRST,
        channel: Channel::FIRST,
        controller,
        value,
    }
}

fn key(key: u8, id: Option<NoteId>) -> NoteKey {
    NoteKey {
        group: Group::FIRST,
        channel: Channel::FIRST,
        key: Key::new(key),
        id,
    }
}

fn send(voices: &mut Poly<Probe, Mono>, messages: &[Message<'_>]) {
    for message in messages {
        voices.handle(message);
    }
}

/// Renders a frame, so every sounding voice reads its expression.
fn render(voices: &mut Poly<Probe, Mono>) {
    let mut buffer = AudioBuf::<Mono>::new(1);
    voices.render(&mut buffer.audio());
}

/// The voice playing note `id`.
fn playing(voices: &mut Poly<Probe, Mono>, id: u32) -> &Probe {
    render(voices);
    let mut found = voices
        .voices_mut()
        .filter(|voice| voice.note.is_some_and(|note| note.id == NoteId::own(id)));
    let voice = found.next().expect("a voice plays the note");
    assert!(found.next().is_none(), "one voice plays it");
    voice
}

fn sounding(voices: &Poly<Probe, Mono>) -> Vec<u8> {
    voices.notes().map(|note| note.key.wire()).collect()
}

#[test]
fn a_note_takes_an_idle_voice_and_its_note_off_releases_it() {
    let mut voices = poly(2);
    send(&mut voices, &[on(60, 1)]);
    assert_eq!(sounding(&voices), [60]);
    assert!(playing(&mut voices, 1).released.is_none());
    send(&mut voices, &[off(60, 1)]);
    assert_eq!(
        playing(&mut voices, 1).released.map(|off| off.velocity),
        Some(0.5)
    );
    // It sounds until it is done.
    voices.voices_mut().for_each(|voice| voice.done = true);
    render(&mut voices);
    assert!(voices.is_silent());
}

#[test]
fn a_full_poly_takes_a_released_voice_then_a_pedal_held_one_then_the_oldest() {
    let mut voices = poly(3);
    send(&mut voices, &[on(60, 1), on(62, 2), on(64, 3), off(62, 2)]);
    send(&mut voices, &[control(number::SUSTAIN, 1.0), off(64, 3)]);
    // 62 is released, 64 held by the pedal, 60 held by its key.
    send(&mut voices, &[on(65, 4)]);
    assert_eq!(sounding(&voices), [60, 65, 64]);
    send(&mut voices, &[on(67, 5)]);
    assert_eq!(sounding(&voices), [60, 65, 67]);
    // Every key is held now: the oldest goes.
    send(&mut voices, &[on(69, 6)]);
    assert_eq!(sounding(&voices), [69, 65, 67]);
    assert_eq!(
        voices.voices_mut().map(|voice| voice.starts).sum::<usize>(),
        6
    );
}

#[test]
fn the_same_key_twice_sounds_twice_and_each_note_off_ends_its_own() {
    let mut voices = poly(4);
    send(&mut voices, &[on(60, 1), on(60, 2)]);
    assert_eq!(sounding(&voices), [60, 60]);
    send(&mut voices, &[off(60, 1)]);
    assert!(playing(&mut voices, 1).released.is_some());
    assert!(playing(&mut voices, 2).released.is_none());
}

#[test]
fn a_note_only_the_pedal_holds_lets_go_when_its_key_is_struck_again() {
    let mut voices = poly(4);
    send(
        &mut voices,
        &[control(number::SUSTAIN, 1.0), on(60, 1), off(60, 1)],
    );
    assert!(playing(&mut voices, 1).released.is_none());
    send(&mut voices, &[on(60, 2)]);
    assert!(playing(&mut voices, 1).released.is_some());
    assert!(playing(&mut voices, 2).released.is_none());
}

#[test]
fn the_sustain_pedal_holds_until_it_comes_up_and_the_damper_follows_half_pedal() {
    let mut voices = poly(2);
    send(&mut voices, &[on(60, 1)]);
    assert_eq!(playing(&mut voices, 1).damper, 0.0, "its key holds it");
    send(&mut voices, &[control(number::SUSTAIN, 1.0), off(60, 1)]);
    let voice = playing(&mut voices, 1);
    assert!((voice.released, voice.damper) == (None, 0.0));
    // Half pedal: still down, the damper halfway to the string.
    send(&mut voices, &[control(number::SUSTAIN, 0.5)]);
    let voice = playing(&mut voices, 1);
    assert!((voice.released, voice.damper) == (None, 0.5));
    send(&mut voices, &[control(number::SUSTAIN, 0.25)]);
    let voice = playing(&mut voices, 1);
    assert!(voice.released.is_some());
    assert_eq!(voice.damper, 0.75);
}

#[test]
fn the_sostenuto_pedal_holds_only_the_notes_whose_keys_were_down_as_it_went_down() {
    let mut voices = poly(4);
    send(
        &mut voices,
        &[on(60, 1), control(number::SOSTENUTO, 1.0), on(64, 2)],
    );
    send(&mut voices, &[off(60, 1), off(64, 2)]);
    assert!(playing(&mut voices, 1).released.is_none());
    assert_eq!(playing(&mut voices, 1).damper, 0.0);
    assert!(playing(&mut voices, 2).released.is_some());
    send(&mut voices, &[control(number::SOSTENUTO, 0.0)]);
    assert!(playing(&mut voices, 1).released.is_some());
}

#[test]
fn all_sound_off_frees_the_channel_at_once_and_all_notes_off_leaves_the_pedal_its_notes() {
    let mut voices = poly(4);
    send(
        &mut voices,
        &[on(60, 1), on(62, 2), control(number::ALL_SOUND_OFF, 0.0)],
    );
    assert!(voices.is_silent());
    send(
        &mut voices,
        &[on(60, 3), control(number::ALL_NOTES_OFF, 0.0)],
    );
    assert!(playing(&mut voices, 3).released.is_some());
    send(&mut voices, &[control(number::SUSTAIN, 1.0), on(62, 4)]);
    // Mono Mode On ends notes too; the pedal still holds this one.
    send(&mut voices, &[control(126, 0.0)]);
    assert!(playing(&mut voices, 4).released.is_none());
    send(&mut voices, &[control(number::SUSTAIN, 0.0)]);
    assert!(playing(&mut voices, 4).released.is_some());
}

#[test]
fn reset_all_controllers_resets_the_bend_pressure_and_pedals_but_not_the_volume() {
    let mut voices = poly(2);
    let channel = (Group::FIRST, Channel::FIRST);
    send(
        &mut voices,
        &[
            Message::Bend {
                group: channel.0,
                channel: channel.1,
                value: Centred::new(1.0),
            },
            Message::Pressure {
                group: channel.0,
                channel: channel.1,
                value: 0.5,
            },
            Message::NotePressure {
                note: key(60, None),
                value: 0.75,
            },
            control(number::VOLUME, 0.25),
            control(number::SUSTAIN, 1.0),
            on(60, 1),
            off(60, 1),
        ],
    );
    let voice = playing(&mut voices, 1);
    assert_eq!(
        (voice.pitch, voice.pressure, voice.volume),
        (62.0, 0.75, 0.25)
    );
    send(&mut voices, &[control(number::RESET_ALL_CONTROLLERS, 0.0)]);
    let voice = playing(&mut voices, 1);
    assert_eq!(
        (voice.pitch, voice.pressure, voice.volume),
        (60.0, 0.0, 0.25)
    );
    assert!(voice.released.is_some(), "the pedal came up");
}

#[test]
fn a_key_s_per_note_controllers_carry_to_its_next_note_until_it_is_reset() {
    let mut voices = poly(4);
    send(&mut voices, &[on(60, 1)]);
    let note = key(60, Some(NoteId::own(1)));
    send(
        &mut voices,
        &[
            Message::NotePressure { note, value: 0.5 },
            Message::NoteControl {
                note,
                controller: number::BRIGHTNESS,
                value: 1.0,
            },
        ],
    );
    assert_eq!(playing(&mut voices, 1).pressure, 0.5);
    send(&mut voices, &[off(60, 1), on(60, 2)]);
    let voice = playing(&mut voices, 2);
    assert_eq!((voice.pressure, voice.timbre), (0.5, 1.0));
    // A reset with a detach: the sounding note keeps its values, the next
    // starts from the defaults.
    send(
        &mut voices,
        &[Message::NoteManagement {
            note: key(60, Some(NoteId::own(2))),
            detach: true,
            reset: true,
        }],
    );
    assert_eq!(playing(&mut voices, 2).pressure, 0.5);
    send(&mut voices, &[on(60, 3)]);
    assert_eq!(playing(&mut voices, 3).pressure, 0.0);
}

#[test]
fn a_reset_without_a_detach_resets_the_sounding_note_too() {
    let mut voices = poly(2);
    let note = key(60, Some(NoteId::own(1)));
    send(
        &mut voices,
        &[on(60, 1), Message::NotePressure { note, value: 0.5 }],
    );
    send(
        &mut voices,
        &[Message::NoteManagement {
            note,
            detach: false,
            reset: true,
        }],
    );
    assert_eq!(playing(&mut voices, 1).pressure, 0.0);
}

#[test]
fn a_per_note_controller_that_comes_before_its_note_arrives_with_it() {
    let mut voices = poly(2);
    send(
        &mut voices,
        &[
            Message::NotePitch {
                note: key(60, None),
                pitch: Pitch::new(60.5),
            },
            on(60, 1),
        ],
    );
    assert_eq!(playing(&mut voices, 1).pitch, 60.5);
}

#[test]
fn a_note_its_host_names_starts_from_the_defaults_and_keeps_its_own() {
    let mut voices = poly(4);
    let named = |id| NoteOn::new(Key::C4, 1.0).with_id(NoteId::hosts(id));
    send(&mut voices, &[Message::NoteOn(named(7))]);
    let note = key(60, Some(NoteId::hosts(7)));
    send(
        &mut voices,
        &[Message::NoteBend {
            note,
            value: Centred::new(0.5),
        }],
    );
    send(&mut voices, &[Message::NoteOff(named(7).off(0.5))]);
    send(&mut voices, &[Message::NoteOn(named(8))]);
    render(&mut voices);
    let pitches: Vec<f32> = voices
        .voices_mut()
        .filter(|v| v.note.is_some())
        .map(|v| v.pitch)
        .collect();
    assert_eq!(
        pitches,
        [84.0, 60.0],
        "a full bend of 48, halved, on the first alone"
    );
}

#[test]
fn the_pitch_adds_the_bends_and_tunings_to_the_key_s_pitch() {
    let mut voices = poly(1);
    let (group, channel) = (Group::FIRST, Channel::FIRST);
    let registered = |index, value| Message::Registered {
        group,
        channel,
        bank: 0,
        index,
        value,
    };
    send(
        &mut voices,
        &[
            Message::BendRange {
                group,
                channel,
                semitones: 12.0,
            },
            Message::Bend {
                group,
                channel,
                value: Centred::new(0.5),
            },
            // A note's own bend: 24 semitones a full bend.
            registered(7, 24.0 / 128.0),
            // Half a semitone sharp, and 2 semitones up.
            registered(1, 0.75),
            registered(2, 66.0 / 128.0),
            on(60, 1),
            Message::NoteBend {
                note: key(60, Some(NoteId::own(1))),
                value: Centred::new(-0.25),
            },
        ],
    );
    let pitch = playing(&mut voices, 1).pitch;
    assert!(
        (pitch - (60.0 + 6.0 - 6.0 + 0.5 + 2.0)).abs() < 1e-4,
        "{pitch}"
    );
}

#[test]
fn a_pitch_of_the_note_s_own_overrides_the_tuning() {
    let mut voices = poly(2).with_tuning(Tuning::from_fn(|key| {
        Pitch::new(f32::from(key.wire()) + 0.25)
    }));
    send(&mut voices, &[on(60, 1)]);
    assert_eq!(playing(&mut voices, 1).pitch, 60.25);
    let attributed = NoteOn::new(Key::new(62), 1.0)
        .with_id(NoteId::own(2))
        .with_pitch(Pitch::new(61.5));
    send(&mut voices, &[Message::NoteOn(attributed)]);
    assert_eq!(playing(&mut voices, 2).pitch, 61.5);
    send(
        &mut voices,
        &[Message::NotePitch {
            note: key(60, Some(NoteId::own(1))),
            pitch: Pitch::new(59.0),
        }],
    );
    assert_eq!(playing(&mut voices, 1).pitch, 59.0);
}

#[test]
fn a_controller_reads_the_note_s_own_then_its_channel_s_then_its_default() {
    let mut voices = poly(2);
    send(&mut voices, &[on(60, 1)]);
    assert_eq!(playing(&mut voices, 1).volume, controller::VOLUME.default());
    send(&mut voices, &[control(number::VOLUME, 0.5)]);
    assert_eq!(playing(&mut voices, 1).volume, 0.5);
    send(
        &mut voices,
        &[Message::NoteControl {
            note: key(60, Some(NoteId::own(1))),
            controller: number::VOLUME,
            value: 0.25,
        }],
    );
    assert_eq!(playing(&mut voices, 1).volume, 0.25);
}

#[test]
fn each_group_s_channels_are_its_own() {
    let mut voices = poly(2);
    send(
        &mut voices,
        &[
            on(60, 1),
            Message::Bend {
                group: Group::number(2),
                channel: Channel::FIRST,
                value: Centred::new(1.0),
            },
            Message::Control {
                group: Group::number(2),
                channel: Channel::FIRST,
                controller: number::ALL_SOUND_OFF,
                value: 0.0,
            },
        ],
    );
    assert_eq!(playing(&mut voices, 1).pitch, 60.0);
}

#[test]
fn play_starts_a_note_at_the_frame_it_arrives_and_sleeps_once_no_voice_sounds() {
    let mut voices = poly(2);
    let mut packets = EventList::with_capacity(2);
    packets.push(Event::new(4, [0x4090_3C00, 0xFFFF_0000, 0, 0]));
    let mut list = MidiList::new();
    list.read(&packets, 8);
    let mut buffer = AudioBuf::<Mono>::new(8);
    assert_eq!(voices.play((buffer.audio(), list.midi())), Status::Continue);
    assert_eq!(
        buffer.channels()[0],
        [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]
    );
    voices.voices_mut().for_each(|voice| voice.done = true);
    let mut quiet = AudioBuf::<Mono>::new(8);
    assert_eq!(
        voices.play((quiet.audio(), crate::Midi::default())),
        Status::Sleep
    );
}

fn on_at(channel: u8, key: u8, id: u32) -> Message<'static> {
    let mut note = NoteOn::new(Key::new(key), 1.0).with_id(NoteId::own(id));
    note.channel = Channel::number(channel);
    Message::NoteOn(note)
}

fn bend_at(channel: u8, value: f32) -> Message<'static> {
    Message::Bend {
        group: Group::FIRST,
        channel: Channel::number(channel),
        value: Centred::new(value),
    }
}

fn control_at(channel: u8, controller: u8, value: f32) -> Message<'static> {
    Message::Control {
        group: Group::FIRST,
        channel: Channel::number(channel),
        controller,
        value,
    }
}

/// An MPE Configuration Message on `channel` for `members` member channels,
/// as a MIDI 1.0 one upgrades.
fn configuration(channel: u8, members: u8) -> Message<'static> {
    Message::Registered {
        group: Group::FIRST,
        channel: Channel::number(channel),
        bank: 0,
        index: 6,
        value: f32::from(members) / 128.0,
    }
}

#[test]
fn mpe_is_off_until_the_plugin_turns_it_on() {
    let mut voices = poly(2);
    send(&mut voices, &[configuration(1, 15), on_at(2, 60, 1)]);
    send(&mut voices, &[bend_at(2, 0.5), bend_at(1, 1.0)]);
    assert_eq!(
        playing(&mut voices, 1).pitch,
        61.0,
        "channel 2's own range of 2"
    );
}

#[test]
fn a_member_note_bends_by_its_channel_and_its_master_and_answers_its_master_s_pedal() {
    let mut voices = poly(4).with_mpe();
    send(&mut voices, &[on_at(2, 60, 1), on_at(3, 64, 2)]);
    send(&mut voices, &[bend_at(2, 0.5), bend_at(1, 1.0)]);
    assert_eq!(playing(&mut voices, 1).pitch, 60.0 + 24.0 + 2.0);
    assert_eq!(playing(&mut voices, 2).pitch, 64.0 + 2.0);
    send(
        &mut voices,
        &[control_at(1, number::SUSTAIN, 1.0), off(60, 1)],
    );
    // The note-off names the note, and the master's pedal holds it.
    let off_2 = {
        let mut off = NoteOn::new(Key::new(60), 1.0)
            .with_id(NoteId::own(1))
            .off(0.5);
        off.channel = Channel::number(2);
        Message::NoteOff(off)
    };
    send(&mut voices, &[off_2]);
    assert!(playing(&mut voices, 1).released.is_none());
    assert_eq!(playing(&mut voices, 1).damper, 0.0);
    send(&mut voices, &[control_at(1, number::SUSTAIN, 0.0)]);
    assert!(playing(&mut voices, 1).released.is_some());
}

#[test]
fn a_member_reads_a_controller_its_channel_has_not_set_from_its_master() {
    let mut voices = poly(2).with_mpe();
    send(
        &mut voices,
        &[on_at(2, 60, 1), control_at(1, number::VOLUME, 0.5)],
    );
    assert_eq!(playing(&mut voices, 1).volume, 0.5);
    send(&mut voices, &[control_at(2, number::VOLUME, 0.25)]);
    assert_eq!(playing(&mut voices, 1).volume, 0.25);
}

#[test]
fn the_master_s_all_notes_off_ends_the_zone_and_a_member_s_its_own_notes() {
    let mut voices = poly(4).with_mpe();
    send(
        &mut voices,
        &[on_at(2, 60, 1), on_at(3, 62, 2), on_at(4, 64, 3)],
    );
    send(&mut voices, &[control_at(3, number::ALL_NOTES_OFF, 0.0)]);
    assert!(playing(&mut voices, 1).released.is_none());
    assert!(playing(&mut voices, 2).released.is_some());
    send(&mut voices, &[control_at(1, number::ALL_SOUND_OFF, 0.0)]);
    assert!(voices.is_silent());
}

#[test]
fn a_configuration_message_moves_the_zones_and_resets_their_bend_ranges() {
    let mut voices = poly(4).with_mpe();
    let range = |channel, semitones| Message::BendRange {
        group: Group::FIRST,
        channel: Channel::number(channel),
        semitones,
    };
    send(&mut voices, &[range(2, 12.0), range(9, 12.0)]);
    // A lower zone of 7: channel 9 is no member now, and channel 2 is back
    // at 48.
    send(&mut voices, &[configuration(1, 7)]);
    send(
        &mut voices,
        &[on_at(2, 60, 1), on_at(9, 60, 2), bend_at(1, 1.0)],
    );
    send(&mut voices, &[bend_at(2, 0.25), bend_at(9, 0.25)]);
    assert_eq!(playing(&mut voices, 1).pitch, 60.0 + 12.0 + 2.0);
    assert_eq!(playing(&mut voices, 2).pitch, 60.0 + 3.0);
    // An upper zone of 3 under channel 16 takes channels 13 to 15.
    send(
        &mut voices,
        &[configuration(16, 3), on_at(14, 60, 3), bend_at(16, 1.0)],
    );
    assert_eq!(playing(&mut voices, 3).pitch, 62.0);
}
