//! Writes what the plugin sent in a block to the host's output event list as
//! VST3's own events.
//!
//! - A note goes out as note events under the host's `noteId`, or under -1 if
//!   the plugin made it up, and its pressure as a poly pressure event.
//! - A note's registered controllers that VST3's six note expressions stand
//!   for go out as expressions, for a note with a host id, with absolute
//!   pitch as the tuning from the note-on's pitch.
//! - Everything else goes out as MIDI 1.0 controllers, downgraded by the
//!   specification's Default Translation, and a system message as the legacy
//!   controller event under VST3's number for it.
//! - A SysEx message goes out whole, from room allocated when the component
//!   is built.
//!
//! A note's assignable controllers, pitch bend and management, a note's own
//! message for a note without a host id, the timing clock and a reset have no
//! VST3 event, so they are dropped and counted.

// A panic in audio-path code ends the host, so these lints reject indexing,
// unwrap, expect, panic and unchecked arithmetic. A provably safe site gives
// its reason in an `#[expect]`.
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

use ::vst3::ComRef;
use ::vst3::Steinberg::Vst::{
    ControllerNumbers_, DataEvent, Event as VstEvent, IEventList, IEventListTrait,
    LegacyMIDICCOutEvent, NoteExpressionValueEvent, NoteOffEvent, NoteOnEvent, PolyPressureEvent,
};
use ::vst3::Steinberg::{int8, int32, kResultOk, uint8};
use hapto_ump::{
    ChannelMessage, Downgrade, Event, MessageType, Midi1Message, Midi1Voice, Origin, SysExJoin,
    scale,
};

use super::super::events::{
    DATA, EXPRESSION, LEGACY_CC_OUT, NOTE_OFF, NOTE_ON, POLY_PRESSURE, SYSEX, blank,
};
use super::midi::{Expression, Named, Notes};

/// VST3's numbers for the MIDI 1.0 messages it carries as controllers.
const PRESSURE: uint8 = ControllerNumbers_::kAfterTouch as uint8;
const BEND: uint8 = ControllerNumbers_::kPitchBend as uint8;
const PROGRAM: uint8 = ControllerNumbers_::kCtrlProgramChange as uint8;

/// What the component keeps from block to block to send a plugin's MIDI.
pub(crate) struct Out {
    /// The notes the plugin sent under a host's id, with their key and
    /// tuning, so a tuning goes out from its note-on's pitch.
    notes: Notes,
    /// A SysEx message being joined, which can run across blocks.
    joining: SysExJoin,
    /// The block's whole SysEx messages, which the host can read until
    /// `process` returns, so they are cleared at the next block.
    block: Vec<u8>,
    /// How many messages VST3 had no event for, or the host refused, since
    /// `Out` was made.
    lost: u64,
}

impl Out {
    /// Allocates room for a plugin that `sends` MIDI, or none.
    pub(crate) fn new(sends: bool) -> Self {
        let room = |bytes| if sends { bytes } else { 0 };
        Out {
            notes: Notes::new(),
            joining: SysExJoin::with_room(room(hapto_midi::SYSEX_BYTES)),
            block: Vec::with_capacity(room(hapto_midi::BLOCK_SYSEX_BYTES)),
            lost: 0,
        }
    }

    /// Clears the notes and any SysEx message under way, as at an activation.
    pub(crate) fn clear(&mut self) {
        self.notes.clear();
        self.joining.reset();
    }

    /// Returns how many messages VST3 had no event for, or the host refused,
    /// since `Out` was made.
    #[cfg(test)]
    pub(crate) fn lost(&self) -> u64 {
        self.lost
    }
}

impl std::fmt::Debug for Out {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Out")
            .field("lost", &self.lost)
            .finish_non_exhaustive()
    }
}

/// Writes `sent`, the plugin's packets from a block, to `events` in order.
///
/// # Safety
///
/// `events` is alive until this call returns, as the SDK promises for the
/// output events `process` receives.
pub(super) unsafe fn write_events(events: ComRef<'_, IEventList>, sent: &[Event], out: &mut Out) {
    out.block.clear();
    let mut add = |mut event: VstEvent| {
        // SAFETY: the caller's contract. The host copies the event, and a
        // SysEx message's bytes stay in `out.block` until the next block.
        unsafe { events.addEvent(&mut event) == kResultOk }
    };
    let downgrade = Downgrade::new();
    let mut lost = 0_u64;
    for packet in sent {
        // Every packet goes through the join, so a packet on a SysEx
        // message's group ends it.
        let joined = out.joining.push(packet);
        let mut dropped = joined.dropped;
        if packet.message_type() == MessageType::SYSEX7 {
            if let Some(whole) = joined.whole
                && !sysex(&mut out.block, whole.framed(), packet.offset, &mut add)
            {
                dropped = dropped.saturating_add(1);
            }
        } else {
            let not_sent = match native(packet, &mut out.notes) {
                Some(Native::Event(event)) => u32::from(!add(event)),
                Some(Native::Lost) => 1,
                None => {
                    let mut refused = 0_u32;
                    let cannot = downgrade.translate(packet, |midi1| {
                        let sent = match midi_1(&midi1) {
                            Some(event) => add(event),
                            None => false,
                        };
                        if !sent {
                            refused = refused.saturating_add(1);
                        }
                    });
                    cannot.saturating_add(refused)
                }
            };
            dropped = dropped.saturating_add(not_sent);
        }
        lost = lost.saturating_add(u64::from(dropped));
    }
    out.lost = out.lost.saturating_add(lost);
}

/// Returns the host's id for the packet's note, or -1 for none, as in VST3.
fn note_id(packet: &Event) -> int32 {
    match packet.origin {
        Origin::Note { id } => int32::try_from(id).unwrap_or(-1),
        _ => -1,
    }
}

/// What a MIDI 2.0 packet becomes in VST3's own events.
enum Native {
    /// VST3's event for it.
    Event(VstEvent),
    /// Nothing, because VST3 has no event for it or no note to put it on.
    Lost,
}

/// Converts a MIDI 2.0 note, a note's pressure or its own message to a VST3
/// event, or returns `None` for another packet, which goes out downgraded.
fn native(packet: &Event, notes: &mut Notes) -> Option<Native> {
    let voice = packet.channel_voice()?;
    let (offset, channel, id) = (packet.offset, voice.channel, note_id(packet));
    Some(match voice.message {
        ChannelMessage::NoteOn {
            note,
            velocity,
            attribute,
        } => {
            let cents = attribute
                .semitones()
                .map_or(0.0, |pitch| ((pitch - f64::from(note)) * 100.0) as f32);
            if id >= 0 {
                notes.start(Named {
                    id,
                    channel,
                    key: note,
                    cents,
                });
            }
            // A host that converts it to MIDI 1.0 rounds a velocity below
            // half a step to 0, a note-off, so the velocity goes out at least
            // at MIDI 1.0's lowest step, as the Default Translation does.
            let velocity = (scale::unit_of_16(velocity) as f32).max(1.0 / 127.0);
            Native::Event(note_on(offset, channel, note, velocity, cents, id))
        }
        ChannelMessage::NoteOff { note, velocity, .. } => {
            let cents = notes.find(id).map_or(0.0, |named| named.cents);
            if id >= 0 {
                notes.end(id);
            }
            let velocity = scale::unit_of_16(velocity) as f32;
            Native::Event(note_off(offset, channel, note, velocity, cents, id))
        }
        ChannelMessage::PolyPressure { note, value } => {
            let pressure = scale::unit_of_32(value) as f32;
            Native::Event(poly_pressure(offset, channel, note, pressure, id))
        }
        ChannelMessage::RegisteredPerNoteController { note, index, value } => {
            match expression(id, note, index, value, notes) {
                Some(value) => {
                    let mut event = blank(offset, EXPRESSION);
                    event.__field0.noteExpressionValue = value;
                    Native::Event(event)
                }
                None => Native::Lost,
            }
        }
        // VST3 has no event for these.
        ChannelMessage::AssignablePerNoteController { .. }
        | ChannelMessage::PerNotePitchBend { .. }
        | ChannelMessage::PerNoteManagement { .. } => Native::Lost,
        _ => return None,
    })
}

/// Returns a note-on with velocity from 0 to 1, tuning in cents from its
/// key, and the host's `id`, or -1 for none.
fn note_on(offset: u32, channel: u8, pitch: u8, velocity: f32, tuning: f32, id: int32) -> VstEvent {
    let mut event = blank(offset, NOTE_ON);
    event.__field0.noteOn = NoteOnEvent {
        channel: i16::from(channel),
        pitch: i16::from(pitch),
        tuning,
        velocity,
        length: 0,
        noteId: id,
    };
    event
}

/// Returns a note-off, with the arguments that [`note_on`] takes.
fn note_off(
    offset: u32,
    channel: u8,
    pitch: u8,
    velocity: f32,
    tuning: f32,
    id: int32,
) -> VstEvent {
    let mut event = blank(offset, NOTE_OFF);
    event.__field0.noteOff = NoteOffEvent {
        channel: i16::from(channel),
        pitch: i16::from(pitch),
        velocity,
        noteId: id,
        tuning,
    };
    event
}

/// Returns a poly pressure event, with `pressure` from 0 to 1.
fn poly_pressure(offset: u32, channel: u8, pitch: u8, pressure: f32, id: int32) -> VstEvent {
    let mut event = blank(offset, POLY_PRESSURE);
    event.__field0.polyPressure = PolyPressureEvent {
        channel: i16::from(channel),
        pitch: i16::from(pitch),
        pressure,
        noteId: id,
    };
    event
}

/// Returns a legacy controller output event under VST3's `number` for a
/// MIDI 1.0 controller or system message, with values masked to 7 bits.
fn legacy_cc(offset: u32, channel: u8, number: uint8, value: u8, value2: u8) -> VstEvent {
    let mut event = blank(offset, LEGACY_CC_OUT);
    event.__field0.midiCCOut = LegacyMIDICCOutEvent {
        controlNumber: number,
        channel: int8::try_from(channel).unwrap_or(0),
        value: int8::try_from(value & 0x7F).unwrap_or(0),
        value2: int8::try_from(value2 & 0x7F).unwrap_or(0),
    };
    event
}

/// Converts a note's registered controller to its note expression, or
/// returns `None` if the note has no host id or the controller no expression.
fn expression(
    id: int32,
    note: u8,
    index: u8,
    value: u32,
    notes: &Notes,
) -> Option<NoteExpressionValueEvent> {
    if id < 0 {
        return None;
    }
    let expression = Expression::by_controller(index)?;
    let cents = notes.find(id).map_or(0.0, |named| named.cents);
    Some(NoteExpressionValueEvent {
        typeId: expression.id,
        noteId: id,
        value: expression.value(value, note, cents),
    })
}

/// Converts a packet from the downgrade to its VST3 event, or returns `None`
/// if VST3 has none.
///
/// MIDI 1.0 becomes note events and controllers, and a system message its
/// controller.
fn midi_1(packet: &Event) -> Option<VstEvent> {
    match packet.message_type() {
        MessageType::MIDI1 => midi_1_event(packet.offset, packet.midi1_voice()?),
        MessageType::SYSTEM => system_event(packet.offset, packet.words[0]),
        _ => None,
    }
}

/// Converts a system message, from its packet's first word, to the legacy
/// controller event under VST3's number for it.
///
/// Returns `None` for the timing clock, a reset and anything undefined,
/// which VST3 has no number for.
fn system_event(offset: u32, word: u32) -> Option<VstEvent> {
    let [_, status, first, second] = word.to_be_bytes();
    let (first, second) = (first & 0x7F, second & 0x7F);
    let (number, value, value2) = match status {
        0xF1 => (ControllerNumbers_::kCtrlQuarterFrame, first, 0),
        // The song position's low 7 bits go first, as the pitch bend's do.
        0xF2 => (ControllerNumbers_::kSystemSongPointer, first, second),
        0xF3 => (ControllerNumbers_::kSystemSongSelect, first, 0),
        0xF6 => (ControllerNumbers_::kSystemTuneRequest, 0, 0),
        0xFA => (ControllerNumbers_::kSystemMidiClockStart, 0, 0),
        0xFB => (ControllerNumbers_::kSystemMidiClockContinue, 0, 0),
        0xFC => (ControllerNumbers_::kSystemMidiClockStop, 0, 0),
        0xFE => (ControllerNumbers_::kSystemActiveSensing, 0, 0),
        _ => return None,
    };
    Some(legacy_cc(offset, 0, number as uint8, value, value2))
}

/// Converts a MIDI 1.0 message to its VST3 event, or returns `None` if VST3
/// doesn't carry it.
fn midi_1_event(offset: u32, voice: Midi1Voice) -> Option<VstEvent> {
    let channel = voice.channel;
    let unit = |value: u8| f32::from(value) / 127.0;
    let cc =
        |number: uint8, value: u8, value2: u8| legacy_cc(offset, channel, number, value, value2);
    Some(match voice.message {
        // A MIDI 1.0 note-on at velocity 0 is a note-off.
        Midi1Message::NoteOn { note, velocity: 0 } => note_off(offset, channel, note, 0.0, 0.0, -1),
        Midi1Message::NoteOn { note, velocity } => {
            note_on(offset, channel, note, unit(velocity), 0.0, -1)
        }
        Midi1Message::NoteOff { note, velocity } => {
            note_off(offset, channel, note, unit(velocity), 0.0, -1)
        }
        Midi1Message::PolyPressure { note, value } => {
            poly_pressure(offset, channel, note, unit(value), -1)
        }
        Midi1Message::ControlChange { controller, value } => cc(controller, value, 0),
        Midi1Message::ChannelPressure { value } => cc(PRESSURE, value, 0),
        // VST3 takes the bend's low 7 bits first.
        Midi1Message::PitchBend { value } => cc(BEND, (value & 0x7F) as u8, (value >> 7) as u8),
        Midi1Message::ProgramChange { program } => cc(PROGRAM, program, 0),
        _ => return None,
    })
}

/// Sends a whole SysEx message, framed by F0 and F7, from the block's room,
/// and returns false if it doesn't fit or the host refuses it.
fn sysex(
    block: &mut Vec<u8>,
    framed: &[u8],
    offset: u32,
    add: &mut impl FnMut(VstEvent) -> bool,
) -> bool {
    let start = block.len();
    if start.saturating_add(framed.len()) > block.capacity() {
        return false;
    }
    // The message fits, so this never allocates.
    block.extend_from_slice(framed);
    let whole = block.get(start..).unwrap_or(&[]);
    let mut event = blank(offset, DATA);
    event.__field0.data = DataEvent {
        size: u32::try_from(whole.len()).unwrap_or(0),
        r#type: SYSEX,
        bytes: whole.as_ptr(),
    };
    add(event)
}
