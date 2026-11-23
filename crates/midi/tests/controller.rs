//! Controllers read typed: MIDI 1.0's from a keyboard's own values, each
//! from its own message alone, a registered, an assignable and a note's
//! own, and each sent and read back as it was.

use hapto_midi::controller::{self, Controller};
use hapto_midi::{Centred, Channel, Decoder, Group, Message, MidiOut, Pedal};
use hapto_ump::{Event, EventList, Midi1Message, Midi1Voice, Upgrade};

/// A MIDI 1.0 control change, upgraded as a keyboard's is, then decoded.
fn sent(controller: u8, value: u8) -> Message<'static> {
    let voice = Midi1Voice {
        group: 0,
        channel: 0,
        message: Midi1Message::ControlChange { controller, value },
    };
    let mut upgraded = EventList::with_capacity(4);
    Upgrade::new().event(&voice.event(0), &mut upgraded);
    decode(upgraded.as_slice()[0])
}

/// The one message `event` decodes to.
fn decode(event: Event) -> Message<'static> {
    let mut decoder = Decoder::new();
    let message = decoder.decode(&event).next().expect("a message");
    match message {
        Message::SysEx { .. } => panic!("no SysEx here"),
        // A message with no borrow lives as long as asked.
        other => owned(other),
    }
}

fn owned(message: Message<'_>) -> Message<'static> {
    match message {
        Message::Control {
            group,
            channel,
            controller,
            value,
        } => Message::Control {
            group,
            channel,
            controller,
            value,
        },
        Message::Registered {
            group,
            channel,
            bank,
            index,
            value,
        } => Message::Registered {
            group,
            channel,
            bank,
            index,
            value,
        },
        Message::Assignable {
            group,
            channel,
            bank,
            index,
            value,
        } => Message::Assignable {
            group,
            channel,
            bank,
            index,
            value,
        },
        Message::NoteControl {
            note,
            controller,
            value,
        } => Message::NoteControl {
            note,
            controller,
            value,
        },
        other => panic!("not a controller: {other}"),
    }
}

#[test]
fn a_keyboards_values_read_typed_and_its_defaults_as_the_controllers_rest() {
    assert_eq!(controller::PAN.read(&sent(10, 0)), Some(Centred::new(-1.0)));
    assert_eq!(
        controller::PAN.read(&sent(10, 64)),
        Some(Centred::CENTRE),
        "the middle, exactly"
    );
    assert_eq!(
        controller::PAN.read(&sent(10, 127)),
        Some(Centred::new(1.0))
    );
    // A default is what a keyboard's value of it reads as.
    assert_eq!(
        controller::VOLUME.read(&sent(7, 100)),
        Some(controller::VOLUME.default())
    );
    assert_eq!(
        controller::EXPRESSION.read(&sent(11, 127)),
        Some(controller::EXPRESSION.default())
    );
    assert_eq!(
        controller::BRIGHTNESS.read(&sent(74, 64)),
        Some(controller::BRIGHTNESS.default())
    );
}

#[test]
fn a_pedal_is_down_from_the_middle_and_keeps_its_depth_for_half_pedal() {
    let up = controller::SUSTAIN.read(&sent(64, 63)).expect("the damper");
    let half = controller::SUSTAIN.read(&sent(64, 64)).expect("the damper");
    let down = controller::SUSTAIN
        .read(&sent(64, 127))
        .expect("the damper");
    assert!(!up.is_down() && half.is_down() && down.is_down());
    assert_eq!((half.depth(), down), (0.5, Pedal::DOWN));
    // A switch reads as on or off.
    assert_eq!(controller::LEGATO.read(&sent(68, 64)), Some(true));
    assert_eq!(controller::PORTAMENTO.read(&sent(65, 10)), Some(false));
}

#[test]
fn a_controller_reads_its_own_message_and_no_other() {
    let volume = sent(7, 100);
    assert_eq!(controller::PAN.read(&volume), None);
    assert_eq!(controller::SUSTAIN.read(&volume), None);
    let note_on = Message::NoteOn(hapto_midi::NoteOn::new(hapto_midi::Key::C4, 1.0));
    assert_eq!(controller::VOLUME.read(&note_on), None);
    // A note's own brightness is not the channel's.
    let own = decode(Event::new(0, [0x4000_3C4A, u32::MAX, 0, 0]));
    assert_eq!(controller::BRIGHTNESS.read(&own), None);
    let (note, bright) = controller::BRIGHTNESS
        .read_note(&own)
        .expect("a note's brightness");
    assert_eq!((note.key.wire(), bright), (60, Centred::new(1.0)));
}

#[test]
fn registered_and_assignable_controllers_read_by_bank_and_index() {
    // Registered controller 0/1, the fine tuning, at its middle.
    let fine = decode(Event::new(0, [0x4020_0001, 0x8000_0000, 0, 0]));
    assert_eq!(controller::FINE_TUNING.read(&fine), Some(Centred::CENTRE));
    assert_eq!(controller::COARSE_TUNING.read(&fine), None);
    // A plugin's own, on assignable controller 1/7.
    const CUTOFF: Controller<f32> = Controller::assignable(1, 7, 0.5);
    let cutoff = decode(Event::new(0, [0x4030_0107, u32::MAX, 0, 0]));
    assert_eq!(CUTOFF.read(&cutoff), Some(1.0));
    assert_eq!(CUTOFF.default(), 0.5);
}

/// `message` sent through a block's MIDI out and decoded again.
fn round_trip(message: Message<'_>) -> Message<'static> {
    let mut packets = EventList::with_capacity(4);
    MidiOut::new(&mut packets).send(0, message);
    decode(packets.as_slice()[0])
}

#[test]
fn a_controllers_message_reads_back_as_it_was_sent() {
    let (group, channel) = (Group::FIRST, Channel::number(3));
    let pan = round_trip(controller::PAN.message(group, channel, Centred::new(-0.5)));
    let read = controller::PAN.read(&pan).expect("a pan").get();
    assert!((read + 0.5).abs() < 1e-6, "{read}");
    assert!(matches!(pan, Message::Control { channel: c, .. } if c == channel));
    let damper = round_trip(controller::SUSTAIN.message(group, channel, Pedal::DOWN));
    assert_eq!(controller::SUSTAIN.read(&damper), Some(Pedal::DOWN));
    let legato = round_trip(controller::LEGATO.message(group, channel, true));
    assert_eq!(controller::LEGATO.read(&legato), Some(true));
    let fine = round_trip(controller::FINE_TUNING.message(group, channel, Centred::new(0.25)));
    let read = controller::FINE_TUNING.read(&fine).expect("a tuning").get();
    assert!((read - 0.25).abs() < 1e-6, "{read}");
}

#[test]
fn a_value_that_is_not_a_number_reads_as_each_types_rest() {
    use hapto_midi::ControllerValue;
    assert_eq!(f32::from_unit(f32::NAN), 0.0);
    assert_eq!(Centred::from_unit(f32::NAN), Centred::CENTRE);
    assert_eq!(Centred::new(f32::NAN), Centred::CENTRE);
    assert_eq!(Pedal::new(f32::NAN), Pedal::UP);
    // In range, each is held to its ends.
    assert_eq!(Centred::from_unit(2.0), Centred::new(1.0));
    assert_eq!(f32::from_unit(-1.0), 0.0);
}
