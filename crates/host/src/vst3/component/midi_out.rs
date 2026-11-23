//! The component's MIDI out: what the plugin sent in a block, written to
//! the host's output event list in VST3's own events.
//!
//! A note goes out as VST3's note events, under the host's `noteId` where
//! the host named it and under none (-1) where the plugin made it up, and a
//! note's pressure as a poly pressure event. A note's own registered
//! controllers that VST3's six note expressions stand for go out as note
//! expressions, for a note its host named: the absolute pitch as the tuning,
//! from the note-on's pitch. VST3 carries everything else only as MIDI 1.0
//! controllers, so it is downgraded by the specification's Default
//! Translation. A SysEx goes out whole, from room the component makes when
//! it is built. What VST3 has no event for is dropped and counted: a note's
//! assignable controllers, its pitch bend and its management, a note's own
//! message for a note no host named, and system messages.

// Audio-path code: a panic here ends a host, so indexing, unwrap, expect,
// panic and unchecked arithmetic are refused, and a site that is provably
// safe says why in an `#[expect]`.
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
    ControllerNumbers_, DataEvent, Event as VstEvent, Event_::EventTypes_, IEventList,
    IEventListTrait, LegacyMIDICCOutEvent, NoteExpressionValueEvent, NoteOffEvent, NoteOnEvent,
    PolyPressureEvent,
};
use ::vst3::Steinberg::{int32, kResultOk, uint8};
use hapto_ump::{ChannelMessage, Downgrade, Event, Midi1Message, Midi1Voice, Origin};

use super::midi::{EXPRESSIONS, Named, Notes, PER_NOTE_PITCH, SYSEX, TUNING_REACH};

/// MIDI 2.0's attribute type for a note-on's pitch, 7.9.
const PITCH_7_9: u8 = 3;
/// VST3's numbers for the MIDI 1.0 messages it carries as controllers.
const PRESSURE: uint8 = ControllerNumbers_::kAfterTouch as uint8;
const BEND: uint8 = ControllerNumbers_::kPitchBend as uint8;
const PROGRAM: uint8 = ControllerNumbers_::kCtrlProgramChange as uint8;

/// What the component keeps to send a plugin's MIDI, from block to block.
pub(crate) struct Out {
    /// The notes the plugin sent under a host's id, with their key and
    /// tuning, so a tuning goes out from its note-on's pitch.
    notes: Notes,
    /// A SysEx being joined, which may span blocks.
    joining: Vec<u8>,
    /// Whether the SysEx being joined outgrew its room, and is dropped.
    over: bool,
    /// The block's whole SysEx messages, which the host may read until
    /// `process` returns, so a block's room is cleared at the next one.
    block: Vec<u8>,
    /// What VST3 had no event for, or the host refused, since it was made.
    lost: u64,
}

impl Out {
    /// Room for a plugin that `sends` MIDI; none for one that does not.
    pub(crate) fn new(sends: bool) -> Self {
        let room = |bytes| if sends { bytes } else { 0 };
        Out {
            notes: Notes::new(),
            joining: Vec::with_capacity(room(hapto_midi::SYSEX_BYTES)),
            over: false,
            block: Vec::with_capacity(room(hapto_midi::BLOCK_SYSEX_BYTES)),
            lost: 0,
        }
    }

    /// Forgets the notes and any SysEx under way, as at an activation.
    pub(crate) fn clear(&mut self) {
        self.notes.clear();
        self.joining.clear();
        self.over = false;
    }

    /// What VST3 had no event for, or the host refused, since it was made.
    #[cfg(test)]
    pub(crate) fn lost(&self) -> u64 {
        self.lost
    }

    fn lose(&mut self) {
        self.lost = self.lost.saturating_add(1);
    }
}

impl std::fmt::Debug for Out {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Out")
            .field("lost", &self.lost)
            .finish_non_exhaustive()
    }
}

/// Writes `sent`, what the plugin sent in a block of `frames` frames, to
/// `events`, in the order the plugin sent it. A packet at or past the
/// block's end goes out at its last frame.
///
/// # Safety
///
/// `events` is alive until this call returns, as the SDK promises for the
/// output events `process` receives.
pub(super) unsafe fn write_events(
    events: ComRef<'_, IEventList>,
    sent: &[Event],
    frames: usize,
    out: &mut Out,
) {
    out.block.clear();
    let last = u32::try_from(frames.saturating_sub(1)).unwrap_or(u32::MAX);
    let mut add = |mut event: VstEvent| {
        // SAFETY: the caller's contract; the host copies the event, and a
        // SysEx's bytes stay in `out.block` until the next block.
        unsafe { events.addEvent(&mut event) == kResultOk }
    };
    let mut downgrade = Downgrade::new();
    for packet in sent {
        let offset = packet.offset.min(last);
        let at = Event { offset, ..*packet };
        if native(&at, out, &mut add) {
            continue;
        }
        downgrade.translate(&at, |midi1| midi_1(&midi1, out, &mut add));
    }
    out.lost = out.lost.saturating_add(downgrade.lost());
}

/// An event of `kind` at `offset` on the event bus, its body set after.
fn event(offset: u32, kind: u16) -> VstEvent {
    // SAFETY: every field of an `Event` is an integer, a float or a raw
    // pointer, for which all zeros is a value.
    let mut event: VstEvent = unsafe { std::mem::zeroed() };
    event.sampleOffset = int32::try_from(offset).unwrap_or(int32::MAX);
    event.r#type = kind;
    event
}

/// The host's name for the packet's note, or -1, VST3's none.
fn note_id(packet: &Event) -> int32 {
    match packet.origin {
        Origin::Note { id } => int32::try_from(id).unwrap_or(-1),
        _ => -1,
    }
}

/// A MIDI 2.0 note, a note's pressure or its own message, in VST3's own
/// events; false for any other packet, which goes out downgraded.
fn native(packet: &Event, out: &mut Out, add: &mut impl FnMut(VstEvent) -> bool) -> bool {
    let Some(voice) = packet.channel_voice() else {
        return false;
    };
    let channel = i16::from(voice.channel);
    let id = note_id(packet);
    let sent = match voice.message {
        ChannelMessage::NoteOn {
            note,
            velocity,
            attribute,
        } => {
            let cents = if attribute.kind == PITCH_7_9 {
                (f32::from(attribute.data) / 512.0 - f32::from(note)) * 100.0
            } else {
                0.0
            };
            if id >= 0 {
                out.notes.start(Named {
                    id,
                    channel: voice.channel,
                    key: note,
                    cents,
                });
            }
            let mut event = event(packet.offset, EventTypes_::kNoteOnEvent as u16);
            event.__field0.noteOn = NoteOnEvent {
                channel,
                pitch: i16::from(note),
                tuning: cents,
                velocity: f32::from(velocity) / f32::from(u16::MAX),
                length: 0,
                noteId: id,
            };
            add(event)
        }
        ChannelMessage::NoteOff { note, velocity, .. } => {
            let cents = out.notes.find(id).map_or(0.0, |named| named.cents);
            if id >= 0 {
                out.notes.end(id);
            }
            let mut event = event(packet.offset, EventTypes_::kNoteOffEvent as u16);
            event.__field0.noteOff = NoteOffEvent {
                channel,
                pitch: i16::from(note),
                velocity: f32::from(velocity) / f32::from(u16::MAX),
                noteId: id,
                tuning: cents,
            };
            add(event)
        }
        ChannelMessage::PolyPressure { note, value } => {
            let mut event = event(packet.offset, EventTypes_::kPolyPressureEvent as u16);
            event.__field0.polyPressure = PolyPressureEvent {
                channel,
                pitch: i16::from(note),
                pressure: (f64::from(value) / f64::from(u32::MAX)) as f32,
                noteId: id,
            };
            add(event)
        }
        ChannelMessage::RegisteredPerNoteController { note, index, value } => {
            match expression(id, note, index, value, &out.notes) {
                Some(value) => {
                    let mut event =
                        event(packet.offset, EventTypes_::kNoteExpressionValueEvent as u16);
                    event.__field0.noteExpressionValue = value;
                    add(event)
                }
                None => false,
            }
        }
        // VST3 has no event for these.
        ChannelMessage::AssignablePerNoteController { .. }
        | ChannelMessage::PerNotePitchBend { .. }
        | ChannelMessage::PerNoteManagement { .. } => false,
        _ => return false,
    };
    if !sent {
        out.lose();
    }
    true
}

/// A note's registered controller as the note expression it stands for,
/// for a note its host named; `None` for any other.
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
    let expression = EXPRESSIONS.iter().find(|kind| kind.controller == index)?;
    let normalized = if index == PER_NOTE_PITCH {
        let cents = notes.find(id).map_or(0.0, |named| named.cents);
        let pitch = f64::from(value) / f64::from(1_u32 << 25);
        let reach = pitch - f64::from(note) - f64::from(cents) / 100.0;
        (reach / TUNING_REACH + 1.0) / 2.0
    } else {
        f64::from(value) / f64::from(u32::MAX)
    };
    Some(NoteExpressionValueEvent {
        typeId: expression.id,
        noteId: id,
        value: normalized.clamp(0.0, 1.0),
    })
}

/// A packet the downgrade sent: MIDI 1.0, which VST3 carries as note
/// events and controllers, or a SysEx packet, or what VST3 has no event
/// for, which is counted.
fn midi_1(packet: &Event, out: &mut Out, add: &mut impl FnMut(VstEvent) -> bool) {
    let sent = match packet.message_type() {
        0x2 => match packet.midi1_voice() {
            Some(voice) => midi_1_event(packet.offset, voice).is_some_and(&mut *add),
            None => false,
        },
        0x3 => {
            sysex(packet, out, add);
            return;
        }
        _ => false,
    };
    if !sent {
        out.lose();
    }
}

/// A MIDI 1.0 message as VST3 carries it; `None` for one it does not.
fn midi_1_event(offset: u32, voice: Midi1Voice) -> Option<VstEvent> {
    let channel = voice.channel;
    let controller = |number: uint8, value: u8, value2: u8| {
        let mut event = event(offset, EventTypes_::kLegacyMIDICCOutEvent as u16);
        event.__field0.midiCCOut = LegacyMIDICCOutEvent {
            controlNumber: number,
            channel: i8::try_from(channel).unwrap_or(0),
            value: i8::try_from(value & 0x7F).unwrap_or(0),
            value2: i8::try_from(value2 & 0x7F).unwrap_or(0),
        };
        event
    };
    let note = |on: bool, note: u8, velocity: u8| {
        let kind = if on {
            EventTypes_::kNoteOnEvent
        } else {
            EventTypes_::kNoteOffEvent
        };
        let mut event = event(offset, kind as u16);
        let velocity = f32::from(velocity) / 127.0;
        let (channel, pitch) = (i16::from(channel), i16::from(note));
        if on {
            event.__field0.noteOn = NoteOnEvent {
                channel,
                pitch,
                tuning: 0.0,
                velocity,
                length: 0,
                noteId: -1,
            };
        } else {
            event.__field0.noteOff = NoteOffEvent {
                channel,
                pitch,
                velocity,
                noteId: -1,
                tuning: 0.0,
            };
        }
        event
    };
    Some(match voice.message {
        // A MIDI 1.0 note-on at velocity 0 is a note-off.
        Midi1Message::NoteOn {
            note: key,
            velocity,
        } => note(velocity > 0, key, velocity),
        Midi1Message::NoteOff {
            note: key,
            velocity,
        } => note(false, key, velocity),
        Midi1Message::PolyPressure { note, value } => {
            let mut event = event(offset, EventTypes_::kPolyPressureEvent as u16);
            event.__field0.polyPressure = PolyPressureEvent {
                channel: i16::from(channel),
                pitch: i16::from(note),
                pressure: f32::from(value) / 127.0,
                noteId: -1,
            };
            event
        }
        Midi1Message::ControlChange {
            controller: number,
            value,
        } => controller(number, value, 0),
        Midi1Message::ChannelPressure { value } => controller(PRESSURE, value, 0),
        // VST3 takes the bend's low 7 bits first.
        Midi1Message::PitchBend { value } => {
            controller(BEND, (value & 0x7F) as u8, (value >> 7) as u8)
        }
        Midi1Message::ProgramChange { program } => controller(PROGRAM, program, 0),
        _ => return None,
    })
}

/// One 7-bit SysEx packet, joined with the ones before it; the whole
/// message goes out once its last packet does, from the block's room.
fn sysex(packet: &Event, out: &mut Out, add: &mut impl FnMut(VstEvent) -> bool) {
    let [head, second, ..] = packet.words;
    let status = (head >> 20) & 0xF;
    let count = (((head >> 16) & 0xF) as usize).min(6);
    let payload = [
        (head >> 8) as u8 & 0x7F,
        head as u8 & 0x7F,
        (second >> 24) as u8 & 0x7F,
        (second >> 16) as u8 & 0x7F,
        (second >> 8) as u8 & 0x7F,
        second as u8 & 0x7F,
    ];
    let bytes = payload.get(..count).unwrap_or(&[]);
    // No room made: a plugin that declares no MIDI out sends none.
    if out.joining.capacity() == 0 {
        out.lose();
        return;
    }
    // A whole or first packet begins a message.
    if matches!(status, 0 | 1) {
        out.joining.clear();
        out.joining.push(0xF0);
        out.over = false;
    }
    if out.joining.is_empty() {
        // A packet of no message begun: nothing to join it to.
        out.lose();
        return;
    }
    // One byte kept for the F7; within the room, so it never allocates.
    if out.joining.len().saturating_add(bytes.len()) < out.joining.capacity() {
        out.joining.extend_from_slice(bytes);
    } else {
        out.over = true;
    }
    if !matches!(status, 0 | 3) {
        return;
    }
    out.joining.push(0xF7);
    let start = out.block.len();
    let fits = start.saturating_add(out.joining.len()) <= out.block.capacity();
    if out.over || !fits {
        out.lose();
    } else {
        out.block.extend_from_slice(&out.joining);
        let whole = out.block.get(start..).unwrap_or(&[]);
        let mut event = event(packet.offset, EventTypes_::kDataEvent as u16);
        event.__field0.data = DataEvent {
            size: u32::try_from(whole.len()).unwrap_or(0),
            r#type: SYSEX,
            bytes: whole.as_ptr(),
        };
        if !add(event) {
            out.lose();
        }
    }
    out.joining.clear();
}
