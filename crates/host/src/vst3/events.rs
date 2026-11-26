//! VST3's event types as an `Event` holds them, and a blank event, for the
//! component's MIDI input and output and the test host.

use ::vst3::Steinberg::Vst::DataEvent_::DataTypes_;
use ::vst3::Steinberg::Vst::Event;
use ::vst3::Steinberg::Vst::Event_::EventTypes_;
use ::vst3::Steinberg::{int32, uint32};

pub(crate) const NOTE_ON: u16 = EventTypes_::kNoteOnEvent as u16;
pub(crate) const NOTE_OFF: u16 = EventTypes_::kNoteOffEvent as u16;
pub(crate) const DATA: u16 = EventTypes_::kDataEvent as u16;
pub(crate) const POLY_PRESSURE: u16 = EventTypes_::kPolyPressureEvent as u16;
pub(crate) const EXPRESSION: u16 = EventTypes_::kNoteExpressionValueEvent as u16;
/// VST3's legacy controller output event, for MIDI 1.0's controllers and
/// system messages.
pub(crate) const LEGACY_CC_OUT: u16 = EventTypes_::kLegacyMIDICCOutEvent as u16;
/// A data event's type for SysEx.
pub(crate) const SYSEX: uint32 = DataTypes_::kMidiSysEx as uint32;

/// Returns an event of `kind` at `offset` on the event bus, its body zeroed
/// for the caller to set, or with both 0 for the host to fill.
pub(crate) fn blank(offset: u32, kind: u16) -> Event {
    // SAFETY: every field of an `Event` is an integer, a float or a raw
    // pointer, for which all zeros is a value.
    let mut event: Event = unsafe { std::mem::zeroed() };
    event.sampleOffset = int32::try_from(offset).unwrap_or(int32::MAX);
    event.r#type = kind;
    event
}
