//! The WinMM backend, which opens Windows' MIDI 1.0 devices through the API
//! that every Windows has.
//!
//! Its system calls are the crate's only `unsafe` code, and each says why it
//! is sound.

mod input;
mod output;
mod signal;
mod watch;

use std::ffi::c_void;

use windows::Win32::Media::Audio::{
    HMIDIIN, HMIDIOUT, MIDIINCAPSW, MIDIOUTCAPSW, midiInGetDevCapsW, midiInGetNumDevs,
    midiInMessage, midiOutGetDevCapsW, midiOutGetNumDevs, midiOutMessage,
};
use windows::Win32::Media::MMSYSERR_NOERROR;
use windows::Win32::Media::Multimedia::{DRV_QUERYDEVICEINTERFACE, DRV_QUERYDEVICEINTERFACESIZE};

pub(crate) use input::Input;
pub(crate) use output::Output;
pub(crate) use signal::Signal;
pub(crate) use watch::Watch;

use crate::error::OpenError;
use crate::id::{Device, DeviceId, Direction, named};

/// How many times, and how far apart, a close retries a driver that is still
/// busy with a buffer, as JUCE does.
const UNPREPARE_TRIES: u32 = 10;
const CLOSE_TRIES: u32 = 5;
const RETRY_WAIT: std::time::Duration = std::time::Duration::from_millis(20);

/// The longest device interface path that is read, in UTF-16 units, as in
/// JUCE.
const PATH_UNITS: usize = 512;

/// Returns every device, inputs first, each in WinMM's order.
pub(crate) fn list() -> Vec<Device> {
    let mut devices: Vec<Device> = listed(Direction::Input)
        .into_iter()
        .map(|(device, _)| device)
        .collect();
    devices.extend(
        listed(Direction::Output)
            .into_iter()
            .map(|(device, _)| device),
    );
    devices
}

/// Returns the devices of `direction`, each with its current WinMM index.
fn listed(direction: Direction) -> Vec<(Device, u32)> {
    let count = match direction {
        // SAFETY: the call takes no arguments and only counts.
        Direction::Input => unsafe { midiInGetNumDevs() },
        // SAFETY: as for the inputs.
        Direction::Output => unsafe { midiOutGetNumDevs() },
    };
    let mut entries = Vec::new();
    let mut indices = Vec::new();
    for index in 0..count {
        let Some(name) = name(direction, index) else {
            continue;
        };
        // A device with no interface path, such as the system's software
        // synth, is keyed by its name.
        let key = interface(direction, index)
            .filter(|path| !path.is_empty())
            .unwrap_or_else(|| name.clone());
        entries.push((name, key));
        indices.push(index);
    }
    named(direction, entries).into_iter().zip(indices).collect()
}

/// Returns the current WinMM index of the device of `direction` that `id`
/// names.
///
/// It reads the list again, because an index moves when a device comes or
/// goes.
pub(crate) fn index_of(direction: Direction, id: &DeviceId) -> Option<u32> {
    listed(direction)
        .into_iter()
        .find(|(device, _)| device.id() == id)
        .map(|(_, index)| index)
}

/// Returns the device's name as WinMM gives it, at most 31 characters.
fn name(direction: Direction, index: u32) -> Option<String> {
    let id = index as usize;
    match direction {
        Direction::Input => {
            let mut caps = MIDIINCAPSW::default();
            let size = size_of::<MIDIINCAPSW>() as u32;
            // SAFETY: `caps` is a MIDIINCAPSW for the call to fill, and
            // `size` is its size.
            let result = unsafe { midiInGetDevCapsW(id, &raw mut caps, size) };
            // A copy, because the struct is packed and its fields can't be
            // borrowed.
            let units = caps.szPname;
            (result == MMSYSERR_NOERROR).then(|| text(&units))
        }
        Direction::Output => {
            let mut caps = MIDIOUTCAPSW::default();
            let size = size_of::<MIDIOUTCAPSW>() as u32;
            // SAFETY: as for an input's.
            let result = unsafe { midiOutGetDevCapsW(id, &raw mut caps, size) };
            let units = caps.szPname;
            (result == MMSYSERR_NOERROR).then(|| text(&units))
        }
    }
}

/// Returns the device's interface path, which stays while the device is
/// plugged in where it is, or an empty string if it has none.
fn interface(direction: Direction, index: u32) -> Option<String> {
    // For these two messages, WinMM takes the device's index for the handle,
    // as DRV_QUERYDEVICEINTERFACE's docs say.
    let device = std::ptr::without_provenance_mut::<c_void>(index as usize);
    let query = |message: u32, first: usize, second: usize| -> u32 {
        match direction {
            // SAFETY: `first` is the address of a u32 for the size query, or
            // of a buffer of `second` bytes for the path, as each message
            // requires.
            Direction::Input => unsafe {
                midiInMessage(Some(HMIDIIN(device)), message, Some(first), Some(second))
            },
            // SAFETY: as for an input.
            Direction::Output => unsafe {
                midiOutMessage(Some(HMIDIOUT(device)), message, Some(first), Some(second))
            },
        }
    };
    let mut bytes: u32 = 0;
    if query(DRV_QUERYDEVICEINTERFACESIZE, (&raw mut bytes) as usize, 0) != MMSYSERR_NOERROR {
        return None;
    }
    let units = (bytes as usize / 2).min(PATH_UNITS);
    if units == 0 {
        return Some(String::new());
    }
    let mut path = vec![0_u16; units];
    let asked = query(
        DRV_QUERYDEVICEINTERFACE,
        path.as_mut_ptr() as usize,
        units * 2,
    );
    (asked == MMSYSERR_NOERROR).then(|| text(&path))
}

/// Decodes UTF-16 up to the first NUL.
fn text(units: &[u16]) -> String {
    let end = units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(units.get(..end).unwrap_or(units))
}

/// How many more times a call that returns `MIDIERR_STILLPLAYING` can wait
/// and retry.
///
/// Calls made in turn can share one budget.
struct Waits(u32);

impl Waits {
    /// Calls `call`, waiting and retrying while it returns
    /// `MIDIERR_STILLPLAYING` and the budget lasts, and returns whether it
    /// succeeded.
    fn retry(&mut self, mut call: impl FnMut() -> u32) -> bool {
        loop {
            match call() {
                MMSYSERR_NOERROR => return true,
                windows::Win32::Media::Audio::MIDIERR_STILLPLAYING if self.0 > 0 => {
                    self.0 = self.0.saturating_sub(1);
                    std::thread::sleep(RETRY_WAIT);
                }
                _ => return false,
            }
        }
    }
}

/// Returns `Ok` if `code`, a WinMM result, means success, or else the error
/// that it means.
fn check(code: u32) -> Result<(), OpenError> {
    if code == MMSYSERR_NOERROR {
        Ok(())
    } else {
        Err(refusal(code))
    }
}

/// Returns the error that a WinMM error code means.
fn refusal(code: u32) -> OpenError {
    use windows::Win32::Media::{MMSYSERR_ALLOCATED, MMSYSERR_BADDEVICEID, MMSYSERR_NOMEM};
    match code {
        MMSYSERR_ALLOCATED => OpenError::InUse,
        MMSYSERR_BADDEVICEID => OpenError::Gone,
        MMSYSERR_NOMEM => OpenError::NoMemory,
        other => OpenError::System(other),
    }
}
