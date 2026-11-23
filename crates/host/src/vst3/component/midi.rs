//! The component's MIDI in: the host's events, and its controllers through
//! hidden proxy parameters, as Universal MIDI Packets for the engine.
//!
//! VST3 hands notes over as events, which become MIDI 2.0, each note named
//! by the host's `noteId` in the packet's [`Origin::Note`], so the decoder
//! names it that way too. A note expression names its note alone, so the
//! component keeps the channel and key of each note its host named.
//!
//! A controller reaches a plugin only as a parameter change, so the
//! component lists a hidden proxy for each channel and controller, 16 by
//! 130, in the id range no key's hash takes, and tells the host through
//! `IMidiMapping`. A proxy's value is its MIDI 1.0 controller's, which the
//! host divided by 127, or by 16383 for the bend, so the component rounds it
//! back to those bits and the engine upgrades it as it does any MIDI 1.0:
//! learn then narrows it exactly. Each controller comes in a queue of its
//! own, so at one frame a parameter's address goes first, and its data
//! entry finds it whatever order the queues come in.

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
    ControllerNumbers_, DataEvent_::DataTypes_, Event, Event_::EventTypes_, IEventList,
    IEventListTrait, IMidiMappingTrait, INoteExpressionControllerTrait, IParamValueQueueTrait,
    IParameterChanges, IParameterChangesTrait, NoteExpressionTypeIDs_, NoteExpressionTypeInfo,
    NoteExpressionTypeInfo_::NoteExpressionTypeFlags_, NoteExpressionValue,
    NoteExpressionValueEvent, NoteOnEvent, ParamID, ParameterInfo, ParameterInfo_::ParameterFlags_,
    String128, TChar, kRootUnitId,
};
use ::vst3::Steinberg::{
    int16, int32, kInternalError, kInvalidArgument, kResultFalse, kResultOk, kResultTrue, tresult,
    uint32,
};
use hapto_midi::controller;
use hapto_params::RESERVED_HOST_IDS;
use hapto_ump::{
    Attribute, ByteStream, ChannelMessage, ChannelVoice, Midi1Message, Midi1Voice, Origin,
};

use super::Component;
use crate::Plugin;
use crate::engine::{MidiIn, Rank};
use crate::vst3::text::{copy_utf16, read_utf16};

/// The channels the grid covers.
const CHANNELS: u32 = 16;
/// Each channel's controllers, as VST3 numbers them: MIDI 1.0's 128, then
/// channel pressure and the pitch bend.
const CONTROLLERS: u32 = ControllerNumbers_::kCountCtrlNumber as uint32;
/// VST3's number for channel pressure.
pub(super) const PRESSURE: u32 = ControllerNumbers_::kAfterTouch as uint32;
/// VST3's number for the pitch bend.
pub(super) const BEND: u32 = ControllerNumbers_::kPitchBend as uint32;
/// How many proxies the grid has.
pub(super) const GRID: u32 = CHANNELS * CONTROLLERS;
/// The first proxy's id, the reserved range's first.
const FIRST_PROXY: u32 = *RESERVED_HOST_IDS.start();
const _: () = assert!(FIRST_PROXY + (GRID - 1) <= *RESERVED_HOST_IDS.end());

/// The most events one block's list is read for: far past any block's
/// room, so a host that claims millions is walked a bounded way.
const MOST_EVENTS: int32 = 1 << 16;

/// The event types a plugin reads, as `Event::type` holds them.
const NOTE_ON: u16 = EventTypes_::kNoteOnEvent as u16;
const NOTE_OFF: u16 = EventTypes_::kNoteOffEvent as u16;
const DATA: u16 = EventTypes_::kDataEvent as u16;
const POLY_PRESSURE: u16 = EventTypes_::kPolyPressureEvent as u16;
const EXPRESSION: u16 = EventTypes_::kNoteExpressionValueEvent as u16;
/// A data event's type for SysEx.
pub(super) const SYSEX: u32 = DataTypes_::kMidiSysEx as uint32;

/// MIDI 2.0's attribute type for a note-on's pitch, 7.9.
const PITCH_7_9: u8 = 3;
/// MIDI 2.0's registered per-note controller for a note's absolute pitch,
/// 7.25.
pub(super) const PER_NOTE_PITCH: u8 = 3;
/// MIDI 1.0's controller for vibrato depth, sound controller 8, which a
/// note's vibrato becomes.
const VIBRATO_DEPTH: u8 = 77;
/// How far VST3's tuning expression reaches either way, in semitones.
pub(super) const TUNING_REACH: f64 = 120.0;

/// The note expressions the component offers, in the order it lists them:
/// each with its title, the per-note controller it becomes, its default and
/// its flags.
pub(super) const EXPRESSIONS: [Expression; 6] = [
    Expression {
        id: NoteExpressionTypeIDs_::kVolumeTypeID,
        title: "Volume",
        controller: controller::number::VOLUME,
        default: 0.25,
        flags: 0,
    },
    Expression {
        id: NoteExpressionTypeIDs_::kPanTypeID,
        title: "Pan",
        controller: controller::number::PAN,
        default: 0.5,
        flags: NoteExpressionTypeFlags_::kIsBipolar as int32,
    },
    Expression {
        id: NoteExpressionTypeIDs_::kTuningTypeID,
        title: "Tuning",
        controller: PER_NOTE_PITCH,
        default: 0.5,
        flags: NoteExpressionTypeFlags_::kIsBipolar as int32,
    },
    Expression {
        id: NoteExpressionTypeIDs_::kVibratoTypeID,
        title: "Vibrato",
        controller: VIBRATO_DEPTH,
        default: 0.0,
        flags: 0,
    },
    Expression {
        id: NoteExpressionTypeIDs_::kExpressionTypeID,
        title: "Expression",
        controller: controller::number::EXPRESSION,
        default: 1.0,
        flags: 0,
    },
    Expression {
        id: NoteExpressionTypeIDs_::kBrightnessTypeID,
        title: "Brightness",
        controller: controller::number::BRIGHTNESS,
        default: 0.5,
        flags: 0,
    },
];

/// A note expression the component offers.
pub(super) struct Expression {
    pub(super) id: u32,
    title: &'static str,
    /// The registered per-note controller it becomes: 3, a note's absolute
    /// pitch, for the tuning.
    pub(super) controller: u8,
    default: f64,
    flags: int32,
}

impl Expression {
    fn of(id: u32) -> Option<&'static Expression> {
        EXPRESSIONS.iter().find(|expression| expression.id == id)
    }
}

/// The proxy for `controller` on `channel`, as VST3 numbers them.
pub(super) fn proxy_id(channel: u32, controller: u32) -> Option<ParamID> {
    if channel >= CHANNELS || controller >= CONTROLLERS {
        return None;
    }
    channel
        .checked_mul(CONTROLLERS)
        .and_then(|row| row.checked_add(controller))
        .and_then(|index| FIRST_PROXY.checked_add(index))
}

/// The channel and controller `id` stands for, if it is a proxy.
pub(super) fn proxy(id: ParamID) -> Option<(u32, u32)> {
    let index = id.checked_sub(FIRST_PROXY).filter(|index| *index < GRID)?;
    Some((
        index.checked_div(CONTROLLERS)?,
        index.checked_rem(CONTROLLERS)?,
    ))
}

/// A proxy's default: the bend at rest, any other controller at 0.
pub(super) fn proxy_default(id: ParamID) -> Option<f64> {
    proxy(id).map(|(_, controller)| if controller == BEND { 0.5 } else { 0.0 })
}

/// Fills `info` for the `index`th proxy, listed after the plugin's own
/// parameters: hidden, so a host shows none of them, and not automatable.
/// False past the grid.
pub(super) fn proxy_info(index: usize, info: &mut ParameterInfo) -> bool {
    let Some(id) = u32::try_from(index)
        .ok()
        .filter(|index| *index < GRID)
        .and_then(|index| FIRST_PROXY.checked_add(index))
    else {
        return false;
    };
    let Some((channel, controller)) = proxy(id) else {
        return false;
    };
    let channel = channel.saturating_add(1);
    let title = match controller {
        PRESSURE => format!("Channel Pressure, channel {channel}"),
        BEND => format!("Pitch Bend, channel {channel}"),
        number => format!("Controller {number}, channel {channel}"),
    };
    info.id = id;
    copy_utf16(&title, &mut info.title);
    copy_utf16(&title, &mut info.shortTitle);
    copy_utf16("", &mut info.units);
    info.stepCount = 0;
    info.defaultNormalizedValue = proxy_default(id).unwrap_or(0.0);
    info.unitId = kRootUnitId;
    info.flags = ParameterFlags_::kIsHidden;
    true
}

/// The notes the host named, each with its channel, key and tuning, so a
/// note expression, which names its note alone, finds them. It has room
/// for as many as the decoder keeps; past that the oldest gives way.
pub(crate) struct Notes {
    named: Vec<Named>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Named {
    pub(super) id: int32,
    pub(super) channel: u8,
    pub(super) key: u8,
    /// The note-on's tuning, in cents.
    pub(super) cents: f32,
}

impl Notes {
    /// Allocates its room.
    pub(crate) fn new() -> Self {
        Notes {
            named: Vec::with_capacity(hapto_midi::NOTES),
        }
    }

    /// Forgets every note, as at an activation.
    pub(crate) fn clear(&mut self) {
        self.named.clear();
    }

    pub(super) fn start(&mut self, note: Named) {
        self.end(note.id);
        if self.named.len() == self.named.capacity() && !self.named.is_empty() {
            self.named.remove(0);
        }
        // Under the capacity, so it never allocates.
        self.named.push(note);
    }

    pub(super) fn end(&mut self, id: int32) {
        self.named.retain(|note| note.id != id);
    }

    pub(super) fn find(&self, id: int32) -> Option<Named> {
        self.named.iter().rev().find(|note| note.id == id).copied()
    }
}

impl std::fmt::Debug for Notes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notes")
            .field("named", &self.named.len())
            .finish()
    }
}

/// Pushes each proxy's points as the MIDI 1.0 controller it stands for,
/// a packet a point at its offset, looking at no more than `most` queues:
/// a parameter's address first at its frame, then the rest in the host's
/// order. Every read, carried or failed, uses up a packet of the block's
/// room, less `keep` packets left for the event list, so a host whose reads
/// fail is read no further than that. Once the room is used up, the points
/// left are counted, unread.
///
/// # Safety
///
/// `changes` and every queue it hands out are alive until this call
/// returns, as the SDK promises for the changes `process` receives.
pub(super) unsafe fn read_controllers(
    changes: ComRef<'_, IParameterChanges>,
    most: usize,
    keep: usize,
    midi: &mut MidiIn<'_>,
) {
    let mut reads = midi.room().saturating_sub(keep);
    // SAFETY: the caller's contract, for each call below.
    let claimed = usize::try_from(unsafe { changes.getParameterCount() }).unwrap_or(0);
    for index in 0..claimed.min(most) {
        let index = int32::try_from(index).unwrap_or(int32::MAX);
        // SAFETY: as above.
        let Some(queue) = (unsafe { ComRef::from_raw(changes.getParameterData(index)) }) else {
            continue;
        };
        // SAFETY: as above.
        let Some((channel, controller)) = proxy(unsafe { queue.getParameterId() }) else {
            continue;
        };
        // SAFETY: as above.
        let points = unsafe { queue.getPointCount() }.max(0);
        for point in 0..points {
            let Some(left) = reads.checked_sub(1) else {
                midi.add_dropped(u32::try_from(points.saturating_sub(point)).unwrap_or(u32::MAX));
                break;
            };
            reads = left;
            let (mut offset, mut value) = (0, 0.0);
            // SAFETY: as above; `offset` and `value` are room for the point.
            if unsafe { queue.getPoint(point, &mut offset, &mut value) } != kResultOk {
                continue;
            }
            let voice = Midi1Voice {
                group: 0,
                channel: u8::try_from(channel).unwrap_or(0),
                message: controller_message(controller, value),
            };
            let event = voice.event(u32::try_from(offset).unwrap_or(0));
            // Each controller comes in a queue of its own. At one frame a
            // parameter's address goes first and its data entry high byte
            // next, so a data entry finds its address and a low byte its
            // high byte whatever order the host lists the queues in.
            let rank = match controller {
                98..=101 => Rank::Address,
                6 => Rank::HighByte,
                _ => Rank::Rest,
            };
            midi.push_ranked(event, rank);
        }
    }
}

/// The MIDI 1.0 message a proxy's value stands for: its controller's 7
/// bits, or the bend's 14, as the host divided them.
fn controller_message(controller: u32, value: f64) -> Midi1Message {
    match controller {
        PRESSURE => Midi1Message::ChannelPressure {
            value: steps(value, 127) as u8,
        },
        BEND => Midi1Message::PitchBend {
            value: steps(value, 0x3FFF) as u16,
        },
        number => Midi1Message::ControlChange {
            controller: u8::try_from(number).unwrap_or(0),
            value: steps(value, 127) as u8,
        },
    }
}

/// `value`, 0 to 1, as the nearest of `most` steps, held to the ends.
fn steps(value: f64, most: u32) -> u32 {
    (value.clamp(0.0, 1.0) * f64::from(most)).round() as u32
}

/// The most a proxy's MIDI 1.0 value reaches: 16383 for the bend, 127 for
/// the rest.
fn most(controller: u32) -> u32 {
    if controller == BEND { 0x3FFF } else { 127 }
}

/// A proxy's `normalized` value as text: the MIDI 1.0 value it stands for,
/// as the engine rounds it. `None` for an id that is no proxy.
pub(super) fn proxy_text(id: ParamID, normalized: f64) -> Option<String> {
    let (_, controller) = proxy(id)?;
    Some(steps(normalized, most(controller)).to_string())
}

/// The normalized value of a proxy's `text`, a MIDI 1.0 value held to its
/// range: `Some(None)` for text that is no whole number, `None` for an id
/// that is no proxy.
pub(super) fn proxy_value(id: ParamID, text: &str) -> Option<Option<f64>> {
    let (_, controller) = proxy(id)?;
    let most = most(controller);
    Some(
        text.trim()
            .parse::<i64>()
            .ok()
            .map(|value| value.clamp(0, i64::from(most)) as f64 / f64::from(most)),
    )
}

/// How many events the host says `events` holds, at most as many as
/// [`read_events`] reads.
///
/// # Safety
///
/// `events` is alive until this call returns.
pub(super) unsafe fn event_count(events: ComRef<'_, IEventList>) -> usize {
    // SAFETY: the caller's contract.
    let count = unsafe { events.getEventCount() }.clamp(0, MOST_EVENTS);
    usize::try_from(count).unwrap_or(0)
}

/// Pushes the host's events on the plugin's event bus as packets: notes,
/// a note's pressure and expression, and SysEx. Chords, scales, text and
/// the rest carry nothing a plugin reads. `notes` is `None` when the
/// main thread holds them, which no host does while it processes; a note
/// expression then finds no note. Once the block has no room, the events
/// left are counted, unread.
///
/// # Safety
///
/// `events` is alive until this call returns, and a SysEx event's bytes
/// with it, as the SDK promises for the events `process` receives.
pub(super) unsafe fn read_events(
    events: ComRef<'_, IEventList>,
    mut notes: Option<&mut Notes>,
    midi: &mut MidiIn<'_>,
) {
    // SAFETY: the caller's contract, for each call below.
    let count = unsafe { events.getEventCount() }.clamp(0, MOST_EVENTS);
    for index in 0..count {
        if !midi.has_room() {
            midi.add_dropped(u32::try_from(count.saturating_sub(index)).unwrap_or(u32::MAX));
            break;
        }
        // SAFETY: every field of an `Event` is an integer, a float or a raw
        // pointer, for which all zeros is a value.
        let mut event: Event = unsafe { std::mem::zeroed() };
        // SAFETY: as above; `event` is room for one.
        if unsafe { events.getEvent(index, &mut event) } != kResultOk || event.busIndex != 0 {
            continue;
        }
        // SAFETY: the caller's contract: a SysEx event's bytes are alive
        // until this call returns.
        unsafe { read_event(&event, notes.as_deref_mut(), midi) };
    }
}

/// One event, as [`read_events`] reads it.
///
/// # Safety
///
/// A SysEx event's bytes are null or valid for reading `size` bytes until
/// this call returns.
unsafe fn read_event(event: &Event, notes: Option<&mut Notes>, midi: &mut MidiIn<'_>) {
    let offset = u32::try_from(event.sampleOffset).unwrap_or(0);
    match event.r#type {
        NOTE_ON => {
            // SAFETY: the type says which of the union's fields is set.
            let on = unsafe { event.__field0.noteOn };
            note_on(&on, offset, notes, midi);
        }
        NOTE_OFF => {
            // SAFETY: as above.
            let off = unsafe { event.__field0.noteOff };
            let Some((channel, key)) = place(off.channel, off.pitch) else {
                return;
            };
            let message = ChannelMessage::NoteOff {
                note: key,
                velocity: sixteen(off.velocity),
                attribute: Attribute::NONE,
            };
            send(midi, offset, channel, off.noteId, message);
            if let Some(notes) = notes.filter(|_| off.noteId >= 0) {
                notes.end(off.noteId);
            }
        }
        POLY_PRESSURE => {
            // SAFETY: as above.
            let pressure = unsafe { event.__field0.polyPressure };
            let Some((channel, key)) = place(pressure.channel, pressure.pitch) else {
                return;
            };
            let message = ChannelMessage::PolyPressure {
                note: key,
                value: bits(f64::from(pressure.pressure)),
            };
            send(midi, offset, channel, pressure.noteId, message);
        }
        EXPRESSION => {
            // SAFETY: as above.
            let value = unsafe { event.__field0.noteExpressionValue };
            if let Some(notes) = notes {
                expression(&value, offset, notes, midi);
            }
        }
        DATA => {
            // SAFETY: as above.
            let data = unsafe { event.__field0.data };
            if data.r#type != SYSEX || data.bytes.is_null() {
                return;
            }
            // SAFETY: this function's contract: `bytes` holds `size` bytes.
            let bytes = unsafe { std::slice::from_raw_parts(data.bytes, data.size as usize) };
            sysex(bytes, offset, midi);
        }
        _ => {}
    }
}

/// Pushes `message` on `channel` at `offset`, its note named `id` by the
/// host, or by no one for a negative `id`, as VST3 says none.
fn send(midi: &mut MidiIn<'_>, offset: u32, channel: u8, id: int32, message: ChannelMessage) {
    let voice = ChannelVoice {
        group: 0,
        channel,
        message,
    };
    let origin = u32::try_from(id).map_or(Origin::Midi2, |id| Origin::Note { id });
    midi.push(hapto_ump::Event {
        origin,
        ..voice.event(offset)
    });
}

/// A note-on, its tuning as its pitch attribute, and its note kept for
/// the expressions that name it.
fn note_on(on: &NoteOnEvent, offset: u32, notes: Option<&mut Notes>, midi: &mut MidiIn<'_>) {
    let Some((channel, key)) = place(on.channel, on.pitch) else {
        return;
    };
    let cents = if on.tuning.is_finite() {
        on.tuning
    } else {
        0.0
    };
    let attribute = if cents == 0.0 {
        Attribute::NONE
    } else {
        let pitch = f64::from(key) + f64::from(cents) / 100.0;
        Attribute {
            kind: PITCH_7_9,
            data: (pitch.clamp(0.0, 127.998) * 512.0).round() as u16,
        }
    };
    let message = ChannelMessage::NoteOn {
        note: key,
        velocity: sixteen(on.velocity),
        attribute,
    };
    send(midi, offset, channel, on.noteId, message);
    if let Some(notes) = notes.filter(|_| on.noteId >= 0) {
        notes.start(Named {
            id: on.noteId,
            channel,
            key,
            cents,
        });
    }
}

/// A note expression, as the registered per-note controller it becomes:
/// the tuning as the note's absolute pitch, from its key and its note-on's
/// tuning. One for a note the host never named is dropped.
fn expression(value: &NoteExpressionValueEvent, offset: u32, notes: &Notes, midi: &mut MidiIn<'_>) {
    let (Some(note), Some(expression)) = (notes.find(value.noteId), Expression::of(value.typeId))
    else {
        return;
    };
    let bits = if expression.controller == PER_NOTE_PITCH {
        let reach = TUNING_REACH * (value.value.clamp(0.0, 1.0) * 2.0 - 1.0);
        let pitch = f64::from(note.key) + f64::from(note.cents) / 100.0 + reach;
        (pitch.clamp(0.0, 127.999_999) * f64::from(1_u32 << 25)).round() as u32
    } else if expression.controller == controller::number::VOLUME {
        bits(volume_of_expression(value.value))
    } else {
        bits(value.value)
    };
    let message = ChannelMessage::RegisteredPerNoteController {
        note: note.key,
        index: expression.controller,
        value: bits,
    };
    send(midi, offset, note.channel, value.noteId, message);
}

/// A SysEx's bytes, with or without its F0 and F7, as 7-bit SysEx packets
/// on the first group.
fn sysex(bytes: &[u8], offset: u32, midi: &mut MidiIn<'_>) {
    let mut stream = ByteStream::new(0);
    let mut send = |words| midi.push(hapto_ump::Event::new(offset, words));
    if bytes.first() != Some(&0xF0) {
        stream.read(&[0xF0], &mut send);
    }
    stream.read(bytes, &mut send);
    if bytes.last() != Some(&0xF7) {
        stream.read(&[0xF7], &mut send);
    }
}

/// A note's channel and key, if both are in range.
fn place(channel: int16, pitch: int16) -> Option<(u8, u8)> {
    let channel = u8::try_from(channel).ok().filter(|channel| *channel < 16)?;
    let key = u8::try_from(pitch).ok().filter(|key| *key < 128)?;
    Some((channel, key))
}

/// A value from 0 to 1 as 16 bits, held to the ends.
fn sixteen(value: f32) -> u16 {
    (value.clamp(0.0, 1.0) * f32::from(u16::MAX)).round() as u16
}

/// A volume note expression's value as the per-note volume it stands for.
/// VST3's value is a gain four times itself, 0 dB at 0.25; the volume
/// follows controller 7's curve, whose gain is its value squared, so 0 dB
/// reads as 1 and −12 dB about as controller 7's 64. VST3's boost past 0 dB
/// is held at 1.
pub(super) fn volume_of_expression(value: f64) -> f64 {
    (4.0 * value.clamp(0.0, 1.0)).sqrt().min(1.0)
}

/// A per-note volume as the volume note expression's value: the inverse of
/// [`volume_of_expression`].
pub(super) fn expression_of_volume(volume: f64) -> f64 {
    volume.clamp(0.0, 1.0).powi(2) / 4.0
}

/// A value from 0 to 1 as 32 bits, held to the ends.
fn bits(value: f64) -> u32 {
    (value.clamp(0.0, 1.0) * f64::from(u32::MAX)).round() as u32
}

/// The controllers a host's MIDI 1.0 input reaches the plugin through: a
/// proxy for each channel and controller on the event bus, as JUCE's
/// wrapper maps them. None for a plugin that takes no MIDI.
impl<P: Plugin> IMidiMappingTrait for Component<P> {
    unsafe fn getMidiControllerAssignment(
        &self,
        bus: int32,
        channel: int16,
        controller: int16,
        id: *mut ParamID,
    ) -> tresult {
        self.guard(kInternalError, || {
            if !P::INFO.midi_input || bus != 0 {
                return kResultFalse;
            }
            let proxy = u32::try_from(channel)
                .ok()
                .zip(u32::try_from(controller).ok())
                .and_then(|(channel, controller)| proxy_id(channel, controller));
            // SAFETY: the SDK's contract: `id` is null or valid for writing
            // one `ParamID` until this call returns.
            match (proxy, unsafe { id.as_mut() }) {
                (Some(proxy), Some(id)) => {
                    *id = proxy;
                    kResultTrue
                }
                (None, _) => kResultFalse,
                (_, None) => kInvalidArgument,
            }
        })
    }
}

/// The note expressions a host may send, on every channel of the event
/// bus, each of which reaches the plugin as a registered per-note
/// controller. None for a plugin that takes no MIDI.
impl<P: Plugin> INoteExpressionControllerTrait for Component<P> {
    unsafe fn getNoteExpressionCount(&self, bus: int32, channel: int16) -> int32 {
        let offered = P::INFO.midi_input && bus == 0 && (0..16).contains(&channel);
        if offered {
            EXPRESSIONS.len() as int32
        } else {
            0
        }
    }

    unsafe fn getNoteExpressionInfo(
        &self,
        bus: int32,
        channel: int16,
        index: int32,
        info: *mut NoteExpressionTypeInfo,
    ) -> tresult {
        self.guard(kInternalError, || {
            // SAFETY: as `getNoteExpressionCount`, which reads nothing.
            let count = unsafe { self.getNoteExpressionCount(bus, channel) };
            let expression = usize::try_from(index)
                .ok()
                .filter(|_| index < count)
                .and_then(|index| EXPRESSIONS.get(index));
            // SAFETY: the SDK's contract: `info` is null or valid for
            // writing one `NoteExpressionTypeInfo` until this call returns.
            let (Some(expression), Some(info)) = (expression, unsafe { info.as_mut() }) else {
                return kInvalidArgument;
            };
            info.typeId = expression.id;
            copy_utf16(expression.title, &mut info.title);
            copy_utf16(expression.title, &mut info.shortTitle);
            let units = if expression.controller == PER_NOTE_PITCH {
                "semitones"
            } else {
                ""
            };
            copy_utf16(units, &mut info.units);
            info.unitId = kRootUnitId;
            info.valueDesc.defaultValue = expression.default;
            info.valueDesc.minimum = 0.0;
            info.valueDesc.maximum = 1.0;
            info.valueDesc.stepCount = 0;
            info.associatedParameterId = 0;
            info.flags = expression.flags;
            kResultOk
        })
    }

    unsafe fn getNoteExpressionStringByValue(
        &self,
        _bus: int32,
        _channel: int16,
        id: u32,
        value: NoteExpressionValue,
        string: *mut String128,
    ) -> tresult {
        self.guard(kInternalError, || {
            // SAFETY: the SDK's contract: `string` is null or valid for
            // writing one `String128` until this call returns.
            let (Some(expression), Some(string)) = (Expression::of(id), unsafe { string.as_mut() })
            else {
                return kInvalidArgument;
            };
            let text = if expression.controller == PER_NOTE_PITCH {
                format!("{:+.2}", TUNING_REACH * (value * 2.0 - 1.0))
            } else {
                format!("{:.0}%", value * 100.0)
            };
            copy_utf16(&text, string);
            kResultOk
        })
    }

    unsafe fn getNoteExpressionValueByString(
        &self,
        _bus: int32,
        _channel: int16,
        id: u32,
        string: *const TChar,
        value: *mut NoteExpressionValue,
    ) -> tresult {
        self.guard(kInternalError, || {
            // SAFETY: the SDK's contract: `value` is null or valid for
            // writing one value until this call returns.
            let (Some(expression), Some(value)) = (Expression::of(id), unsafe { value.as_mut() })
            else {
                return kInvalidArgument;
            };
            // SAFETY: the SDK's contract: `string` is null or a
            // NUL-terminated string of at most a `String128`, readable until
            // this call returns, as `read_utf16` requires.
            let Some(text) = (unsafe { read_utf16(string, 128) }) else {
                return kInvalidArgument;
            };
            let number = text.trim().trim_end_matches('%').trim();
            let Ok(number) = number.parse::<f64>() else {
                return kResultFalse;
            };
            *value = if expression.controller == PER_NOTE_PITCH {
                (number / TUNING_REACH + 1.0) / 2.0
            } else {
                number / 100.0
            }
            .clamp(0.0, 1.0);
            kResultOk
        })
    }
}
