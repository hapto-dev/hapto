//! Turns the host's events, and the controllers it sends through hidden
//! proxy parameters, into Universal MIDI Packets for the engine.
//!
//! Notes become MIDI 2.0, each packet with the host's `noteId` in its
//! [`Origin::Note`], so the decoder identifies notes by that id too. A note
//! expression gives only the id, so the component keeps each such note's
//! channel and key.
//!
//! VST3 sends a controller only as a parameter change, so the component lists
//! a hidden proxy for each channel and controller (16 by 130), in the id range
//! that no key's hash takes, and maps them through `IMidiMapping`. The host
//! divides a proxy's MIDI 1.0 value by 127, or 16383 for the pitch bend. The
//! component rounds it back, and the engine upgrades it like any MIDI 1.0, so
//! learn narrows it back exactly.

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
    ControllerNumbers_, Event, IEventList, IEventListTrait, IMidiMappingTrait,
    INoteExpressionControllerTrait, IParamValueQueueTrait, IParameterChanges,
    IParameterChangesTrait, NoteExpressionTypeIDs_, NoteExpressionTypeInfo,
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
    Attribute, ChannelMessage, ChannelVoice, Midi1Message, Midi1Voice, Origin, SysEx7, scale,
};

use super::super::events::{DATA, EXPRESSION, NOTE_OFF, NOTE_ON, POLY_PRESSURE, SYSEX, blank};
use super::Component;
use crate::Plugin;
use crate::engine::{MidiIn, Rank};
use crate::vst3::text::{copy_utf16, read_utf16};
use hapto_ump::controller::{DATA_ENTRY, NRPN_LSB, RPN_MSB};

/// The channels the grid covers.
const CHANNELS: u32 = 16;
/// Each channel's controllers as VST3 numbers them, MIDI 1.0's 128 then
/// channel pressure and the pitch bend.
const CONTROLLERS: u32 = ControllerNumbers_::kCountCtrlNumber as uint32;
/// VST3's number for channel pressure.
pub(super) const PRESSURE: u8 = ControllerNumbers_::kAfterTouch as u8;
/// VST3's number for the pitch bend.
pub(super) const BEND: u8 = ControllerNumbers_::kPitchBend as u8;
/// How many proxies the grid has.
pub(super) const GRID: u32 = CHANNELS * CONTROLLERS;
/// The first proxy's id, which starts the reserved range.
const FIRST_PROXY: u32 = *RESERVED_HOST_IDS.start();
const _: () = assert!(FIRST_PROXY + (GRID - 1) <= *RESERVED_HOST_IDS.end());

/// The most events read from one block's list, far more than a block has
/// room for, so a host that claims millions is read only this far.
const MOST_EVENTS: int32 = 1 << 16;

/// MIDI 2.0's registered per-note controller for a note's absolute pitch
/// (Pitch 7.25).
pub(super) const PER_NOTE_PITCH: u8 = ChannelMessage::PER_NOTE_PITCH;
/// MIDI 1.0's controller for vibrato depth (sound controller 8), which a
/// note's vibrato becomes.
const VIBRATO_DEPTH: u8 = 77;
/// How far VST3's tuning expression reaches either way, in semitones.
pub(super) const TUNING_REACH: f64 = 120.0;

/// The note expressions that the component offers, in the order it lists
/// them.
pub(super) const EXPRESSIONS: [Expression; 6] = [
    Expression {
        id: NoteExpressionTypeIDs_::kVolumeTypeID,
        title: "Volume",
        controller: controller::number::VOLUME,
        default: 0.25,
        flags: 0,
        curve: Curve::Volume,
    },
    Expression {
        id: NoteExpressionTypeIDs_::kPanTypeID,
        title: "Pan",
        controller: controller::number::PAN,
        default: 0.5,
        flags: NoteExpressionTypeFlags_::kIsBipolar as int32,
        curve: Curve::Linear,
    },
    Expression {
        id: NoteExpressionTypeIDs_::kTuningTypeID,
        title: "Tuning",
        controller: PER_NOTE_PITCH,
        default: 0.5,
        flags: NoteExpressionTypeFlags_::kIsBipolar as int32,
        curve: Curve::Tuning,
    },
    Expression {
        id: NoteExpressionTypeIDs_::kVibratoTypeID,
        title: "Vibrato",
        controller: VIBRATO_DEPTH,
        default: 0.0,
        flags: 0,
        curve: Curve::Linear,
    },
    Expression {
        id: NoteExpressionTypeIDs_::kExpressionTypeID,
        title: "Expression",
        controller: controller::number::EXPRESSION,
        default: 1.0,
        flags: 0,
        curve: Curve::Linear,
    },
    Expression {
        id: NoteExpressionTypeIDs_::kBrightnessTypeID,
        title: "Brightness",
        controller: controller::number::BRIGHTNESS,
        default: 0.5,
        flags: 0,
        curve: Curve::Linear,
    },
];

/// A note expression the component offers.
pub(super) struct Expression {
    pub(super) id: u32,
    title: &'static str,
    /// The registered per-note controller it becomes, such as absolute pitch
    /// (3) for the tuning.
    pub(super) controller: u8,
    default: f64,
    flags: int32,
    curve: Curve,
}

/// How a note expression's value (0 to 1) maps to its controller's and
/// reads to a musician.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Curve {
    /// The tuning, as absolute pitch up to [`TUNING_REACH`] semitones either
    /// side of the note-on's.
    Tuning,
    /// The volume, on controller 7's curve.
    Volume,
    /// The value as it is.
    Linear,
}

impl Expression {
    /// Returns the expression whose VST3 type id is `id`.
    fn by_type(id: u32) -> Option<&'static Expression> {
        EXPRESSIONS.iter().find(|expression| expression.id == id)
    }

    /// Returns the expression for registered per-note controller `index`.
    pub(super) fn by_controller(index: u8) -> Option<&'static Expression> {
        EXPRESSIONS
            .iter()
            .find(|expression| expression.controller == index)
    }

    /// Returns the controller's 32 bits for VST3's `value` on `note`.
    fn bits(&self, value: f64, note: &Named) -> u32 {
        match self.curve {
            Curve::Tuning => {
                let tuned = f64::from(note.key) + f64::from(note.cents) / 100.0;
                scale::pitch_7_25(tuned + semitones_of_tuning(value))
            }
            Curve::Volume => bits(volume_of_expression(value)),
            Curve::Linear => bits(value),
        }
    }

    /// Returns VST3's value for 32 `bits` on a note at `key` whose note-on
    /// was tuned by `cents`.
    pub(super) fn value(&self, bits: u32, key: u8, cents: f32) -> f64 {
        match self.curve {
            Curve::Tuning => {
                let pitch = scale::semitones_7_25(bits);
                tuning_of_semitones(pitch - f64::from(key) - f64::from(cents) / 100.0)
            }
            Curve::Volume => expression_of_volume(scale::unit_of_32(bits)),
            Curve::Linear => scale::unit_of_32(bits),
        }
        .clamp(0.0, 1.0)
    }

    /// Returns the value as text, in semitones for the tuning and percent for
    /// the rest.
    fn text(&self, value: f64) -> String {
        match self.curve {
            Curve::Tuning => format!("{:+.2}", semitones_of_tuning(value)),
            Curve::Volume | Curve::Linear => format!("{:.0}%", value * 100.0),
        }
    }

    /// Parses `text` as [`text`](Self::text) writes it, clamped to 0 to 1, or
    /// returns `None` if it isn't a number.
    fn parse(&self, text: &str) -> Option<f64> {
        let number = text
            .trim()
            .trim_end_matches('%')
            .trim()
            .parse::<f64>()
            .ok()?;
        Some(
            match self.curve {
                Curve::Tuning => tuning_of_semitones(number),
                Curve::Volume | Curve::Linear => number / 100.0,
            }
            .clamp(0.0, 1.0),
        )
    }

    /// Returns its text's units.
    fn units(&self) -> &'static str {
        match self.curve {
            Curve::Tuning => "semitones",
            Curve::Volume | Curve::Linear => "",
        }
    }
}

/// Converts a tuning value (0 to 1) to semitones, up to [`TUNING_REACH`]
/// either way.
fn semitones_of_tuning(value: f64) -> f64 {
    TUNING_REACH * (value.clamp(0.0, 1.0) * 2.0 - 1.0)
}

/// Converts `semitones` to a tuning value, the inverse of
/// [`semitones_of_tuning`].
fn tuning_of_semitones(semitones: f64) -> f64 {
    (semitones / TUNING_REACH + 1.0) / 2.0
}

/// A proxy, the hidden parameter that carries a host's MIDI 1.0 controller
/// on one channel, as JUCE's wrapper maps them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Proxy {
    channel: u8,
    controller: u8,
}

impl Proxy {
    /// Returns the proxy `id` for plugin `P`, which has proxies only if it
    /// takes MIDI.
    pub(super) fn of<P: Plugin>(id: ParamID) -> Option<Proxy> {
        P::INFO.midi_input.then(|| Proxy::named(id)).flatten()
    }

    /// Returns the proxy `id`, if it is one.
    fn named(id: ParamID) -> Option<Proxy> {
        let index = id.checked_sub(FIRST_PROXY).filter(|index| *index < GRID)?;
        Proxy::from_index(index)
    }

    /// Returns the `index`th proxy, listed after the plugin's own parameters.
    pub(super) fn at(index: usize) -> Option<Proxy> {
        Proxy::from_index(u32::try_from(index).ok().filter(|index| *index < GRID)?)
    }

    fn from_index(index: u32) -> Option<Proxy> {
        Some(Proxy {
            channel: u8::try_from(index.checked_div(CONTROLLERS)?).ok()?,
            controller: u8::try_from(index.checked_rem(CONTROLLERS)?).ok()?,
        })
    }

    /// Returns the proxy for `controller` on `channel`, as VST3 numbers them.
    fn mapped(channel: int16, controller: int16) -> Option<Proxy> {
        let channel = u8::try_from(channel)
            .ok()
            .filter(|c| u32::from(*c) < CHANNELS)?;
        let controller = u8::try_from(controller)
            .ok()
            .filter(|c| u32::from(*c) < CONTROLLERS)?;
        Some(Proxy {
            channel,
            controller,
        })
    }

    /// Returns its parameter id.
    pub(super) fn id(self) -> ParamID {
        // `index` is below the grid's size, which fits the reserved range.
        let index = u32::from(self.channel)
            .saturating_mul(CONTROLLERS)
            .saturating_add(u32::from(self.controller));
        FIRST_PROXY.saturating_add(index)
    }

    /// Returns its largest MIDI 1.0 value, 16383 for the pitch bend and 127
    /// for the rest.
    fn most(self) -> u32 {
        if self.controller == BEND { 0x3FFF } else { 127 }
    }

    /// Returns its default, the centre for the pitch bend and 0 for the rest.
    pub(super) fn default(self) -> f64 {
        if self.controller == BEND { 0.5 } else { 0.0 }
    }

    /// Returns the MIDI 1.0 message for `value`, with the 7 bits (14 for the
    /// pitch bend) that the host divided.
    fn message(self, value: f64) -> Midi1Message {
        let steps = steps(value, self.most());
        match self.controller {
            PRESSURE => Midi1Message::ChannelPressure { value: steps as u8 },
            BEND => Midi1Message::PitchBend {
                value: steps as u16,
            },
            controller => Midi1Message::ControlChange {
                controller,
                value: steps as u8,
            },
        }
    }

    /// Returns its message's place among those at its frame, as [`Rank`]
    /// says, with an address first and a data entry high byte next.
    fn rank(self) -> Rank {
        match self.controller {
            NRPN_LSB..=RPN_MSB => Rank::Address,
            DATA_ENTRY => Rank::HighByte,
            _ => Rank::Rest,
        }
    }

    /// Returns `normalized` as text, the MIDI 1.0 value it stands for,
    /// rounded as the engine rounds it.
    pub(super) fn text(self, normalized: f64) -> String {
        steps(normalized, self.most()).to_string()
    }

    /// Parses `text` as a MIDI 1.0 value clamped to its range and returns it
    /// normalized, or `None` if it isn't a whole number.
    pub(super) fn parse(self, text: &str) -> Option<f64> {
        let most = self.most();
        let value = text.trim().parse::<i64>().ok()?;
        Some(value.clamp(0, i64::from(most)) as f64 / f64::from(most))
    }

    /// Fills `info` for it as hidden, so a host shows no proxy, and not
    /// automatable.
    pub(super) fn fill(self, info: &mut ParameterInfo) {
        let channel = u32::from(self.channel).saturating_add(1);
        let title = match self.controller {
            PRESSURE => format!("Channel Pressure, channel {channel}"),
            BEND => format!("Pitch Bend, channel {channel}"),
            number => format!("Controller {number}, channel {channel}"),
        };
        info.id = self.id();
        copy_utf16(&title, &mut info.title);
        copy_utf16(&title, &mut info.shortTitle);
        copy_utf16("", &mut info.units);
        info.stepCount = 0;
        info.defaultNormalizedValue = self.default();
        info.unitId = kRootUnitId;
        info.flags = ParameterFlags_::kIsHidden;
    }
}

/// The notes that the host gave ids, with their channel, key and tuning, for
/// the note expressions that give only an id.
///
/// It holds as many as the decoder keeps and then drops the oldest.
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

    /// Removes every note, as at an activation.
    pub(crate) fn clear(&mut self) {
        self.named.clear();
    }

    pub(super) fn start(&mut self, note: Named) {
        self.end(note.id);
        if self.named.len() == self.named.capacity() && !self.named.is_empty() {
            self.named.remove(0);
        }
        // The push stays within the capacity, so it never allocates.
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

/// Pushes each proxy's points as its MIDI 1.0 controller, a packet per point
/// at its offset, from at most `most` queues, ranked as [`Proxy::rank`] says.
///
/// Every read, even a failed one, uses up a packet of the block's room less
/// `keep` packets left for the event list, so a host whose reads fail is
/// read no further. Once the room is used up, the points left are counted as
/// dropped.
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
        let Some(proxy) = Proxy::named(unsafe { queue.getParameterId() }) else {
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
            // SAFETY: as above, and `offset` and `value` have room for one
            // point.
            if unsafe { queue.getPoint(point, &mut offset, &mut value) } != kResultOk {
                continue;
            }
            let voice = Midi1Voice {
                group: 0,
                channel: proxy.channel,
                message: proxy.message(value),
            };
            let event = voice.event(u32::try_from(offset).unwrap_or(0));
            midi.push_ranked(event, proxy.rank());
        }
    }
}

/// Rounds `value`, clamped to 0 to 1, to the nearest of `most` steps.
fn steps(value: f64, most: u32) -> u32 {
    (value.clamp(0.0, 1.0) * f64::from(most)).round() as u32
}

/// Returns how many events the host reports in `events`, capped at what
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

/// Pushes the host's notes, a note's pressure and expression, and SysEx on
/// the plugin's event bus as packets.
///
/// Chords, scales, text and the rest carry nothing a plugin reads. `notes` is
/// `None` while the main thread holds them, which no host causes during
/// processing, and a note expression then finds no note. Once the block has
/// no room, the events left are counted as dropped.
///
/// # Safety
///
/// `events` and each SysEx event's bytes are alive until this call returns,
/// as the SDK promises for the events that `process` receives.
pub(super) unsafe fn read_events(
    events: ComRef<'_, IEventList>,
    mut notes: Option<&mut Notes>,
    midi: &mut MidiIn<'_>,
) {
    // SAFETY: the caller's contract, for each call below.
    let count = unsafe { event_count(events) };
    for index in 0..count {
        if !midi.has_room() {
            midi.add_dropped(u32::try_from(count.saturating_sub(index)).unwrap_or(u32::MAX));
            break;
        }
        let mut event = blank(0, 0);
        let Ok(index) = int32::try_from(index) else {
            break;
        };
        // SAFETY: as above, and `event` has room for one event.
        if unsafe { events.getEvent(index, &mut event) } != kResultOk || event.busIndex != 0 {
            continue;
        }
        // SAFETY: by the caller's contract, a SysEx event's bytes are alive
        // until this call returns.
        unsafe { read_event(&event, notes.as_deref_mut(), midi) };
    }
}

/// Reads one event, as [`read_events`] does.
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
            // SAFETY: by this function's contract, `bytes` holds `size` bytes.
            let bytes = unsafe { std::slice::from_raw_parts(data.bytes, data.size as usize) };
            sysex(bytes, offset, midi);
        }
        _ => {}
    }
}

/// Pushes `message` on `channel` at `offset` under the host's note `id`,
/// where a negative `id` means none, as in VST3.
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

/// Pushes a note-on with its tuning as its pitch attribute, and keeps the
/// note for its expressions.
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
        Attribute::pitch(f64::from(key) + f64::from(cents) / 100.0)
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

/// Pushes a note expression as its registered per-note controller, with the
/// tuning as absolute pitch from the key and the note-on's tuning.
///
/// An expression for a note without a host id is dropped.
fn expression(value: &NoteExpressionValueEvent, offset: u32, notes: &Notes, midi: &mut MidiIn<'_>) {
    let (Some(note), Some(expression)) =
        (notes.find(value.noteId), Expression::by_type(value.typeId))
    else {
        return;
    };
    let message = ChannelMessage::RegisteredPerNoteController {
        note: note.key,
        index: expression.controller,
        value: expression.bits(value.value, &note),
    };
    send(midi, offset, note.channel, value.noteId, message);
}

/// Pushes a SysEx message's bytes, with or without F0 and F7, as one 7-bit
/// SysEx message on the first group, because a format's SysEx event holds
/// one.
///
/// Each byte is masked to 7 bits.
fn sysex(bytes: &[u8], offset: u32, midi: &mut MidiIn<'_>) {
    let data = bytes.strip_prefix(&[0xF0]).unwrap_or(bytes);
    let data = data.strip_suffix(&[0xF7]).unwrap_or(data);
    SysEx7::packets(0, data, |words| {
        midi.push(hapto_ump::Event::new(offset, words));
    });
}

/// Returns a note's channel and key, if both are in range.
fn place(channel: int16, pitch: int16) -> Option<(u8, u8)> {
    let channel = u8::try_from(channel).ok().filter(|channel| *channel < 16)?;
    let key = u8::try_from(pitch).ok().filter(|key| *key < 128)?;
    Some((channel, key))
}

/// Converts a value from 0 to 1, clamped, to 16 bits.
fn sixteen(value: f32) -> u16 {
    scale::unit_to_16(f64::from(value))
}

/// Converts a volume note expression's value to its per-note volume.
///
/// VST3's gain is four times its value, so 0 dB is at 0.25. The volume
/// follows controller 7's curve, whose gain is the value squared, so 0 dB
/// becomes 1 and −12 dB about controller 7's 64. VST3's boost above 0 dB is
/// clamped to 1.
pub(super) fn volume_of_expression(value: f64) -> f64 {
    (4.0 * value.clamp(0.0, 1.0)).sqrt().min(1.0)
}

/// Converts a per-note volume to a volume expression's value, the inverse of
/// [`volume_of_expression`].
pub(super) fn expression_of_volume(volume: f64) -> f64 {
    volume.clamp(0.0, 1.0).powi(2) / 4.0
}

/// Converts a value from 0 to 1, clamped, to 32 bits.
fn bits(value: f64) -> u32 {
    scale::unit_to_32(value)
}

/// The proxies that carry a host's MIDI 1.0 controllers, one for each
/// channel and controller on the event bus, as JUCE's wrapper maps them, and
/// none for a plugin that takes no MIDI.
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
            // SAFETY: by the SDK's contract, `id` is null or valid for
            // writing one `ParamID` until this call returns.
            match (Proxy::mapped(channel, controller), unsafe { id.as_mut() }) {
                (Some(proxy), Some(id)) => {
                    *id = proxy.id();
                    kResultTrue
                }
                (None, _) => kResultFalse,
                (_, None) => kInvalidArgument,
            }
        })
    }
}

/// Returns whether the component offers note expressions on `bus` and
/// `channel`, as it does on the event bus's 16 channels for a plugin that
/// takes MIDI.
fn offers_expressions<P: Plugin>(bus: int32, channel: int16) -> bool {
    P::INFO.midi_input && bus == 0 && u32::try_from(channel).is_ok_and(|channel| channel < CHANNELS)
}

/// The note expressions that a host may send on every channel of the event
/// bus. Each reaches the plugin as a registered per-note controller, and a
/// plugin that takes no MIDI has none.
impl<P: Plugin> INoteExpressionControllerTrait for Component<P> {
    unsafe fn getNoteExpressionCount(&self, bus: int32, channel: int16) -> int32 {
        if offers_expressions::<P>(bus, channel) {
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
            let expression = usize::try_from(index)
                .ok()
                .filter(|_| offers_expressions::<P>(bus, channel))
                .and_then(|index| EXPRESSIONS.get(index));
            // SAFETY: by the SDK's contract, `info` is null or valid for
            // writing one `NoteExpressionTypeInfo` until this call returns.
            let (Some(expression), Some(info)) = (expression, unsafe { info.as_mut() }) else {
                return kInvalidArgument;
            };
            info.typeId = expression.id;
            copy_utf16(expression.title, &mut info.title);
            copy_utf16(expression.title, &mut info.shortTitle);
            copy_utf16(expression.units(), &mut info.units);
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
        bus: int32,
        channel: int16,
        id: u32,
        value: NoteExpressionValue,
        string: *mut String128,
    ) -> tresult {
        self.guard(kInternalError, || {
            let expression =
                Expression::by_type(id).filter(|_| offers_expressions::<P>(bus, channel));
            // SAFETY: by the SDK's contract, `string` is null or valid for
            // writing one `String128` until this call returns.
            let (Some(expression), Some(string)) = (expression, unsafe { string.as_mut() }) else {
                return kInvalidArgument;
            };
            copy_utf16(&expression.text(value), string);
            kResultOk
        })
    }

    unsafe fn getNoteExpressionValueByString(
        &self,
        bus: int32,
        channel: int16,
        id: u32,
        string: *const TChar,
        value: *mut NoteExpressionValue,
    ) -> tresult {
        self.guard(kInternalError, || {
            let expression =
                Expression::by_type(id).filter(|_| offers_expressions::<P>(bus, channel));
            // SAFETY: by the SDK's contract, `value` is null or valid for
            // writing one value until this call returns.
            let (Some(expression), Some(value)) = (expression, unsafe { value.as_mut() }) else {
                return kInvalidArgument;
            };
            // SAFETY: by the SDK's contract, `string` is null or a
            // NUL-terminated string of at most a `String128`, readable until
            // this call returns, as `read_utf16` requires.
            let Some(text) = (unsafe { read_utf16(string, 128) }) else {
                return kInvalidArgument;
            };
            match expression.parse(&text) {
                Some(parsed) => {
                    *value = parsed;
                    kResultOk
                }
                None => kResultFalse,
            }
        })
    }
}
