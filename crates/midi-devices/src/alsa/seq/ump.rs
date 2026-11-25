//! alsa-lib's UMP calls, looked up when first needed rather than linked.
//!
//! Rust links a Linux binary to bind every function it calls at load time,
//! so a linked UMP call would stop the app from starting on an alsa-lib
//! older than 1.2.10, which has none. A lookup lets the app start there and
//! read MIDI 1.0.

use std::ffi::{CStr, c_int, c_void};
use std::sync::OnceLock;

use alsa_sys as sys;

// The UMP calls' types, as seq.h declares them.
type SetMidiVersion = unsafe extern "C" fn(*mut sys::snd_seq_t, c_int) -> c_int;
type InfoMidiVersion = unsafe extern "C" fn(*const sys::snd_seq_client_info_t) -> c_int;
type EventInput =
    unsafe extern "C" fn(*mut sys::snd_seq_t, *mut *mut sys::snd_seq_ump_event_t) -> c_int;
#[cfg(test)]
type EventOutput =
    unsafe extern "C" fn(*mut sys::snd_seq_t, *mut sys::snd_seq_ump_event_t) -> c_int;

/// The UMP calls that this alsa-lib has.
#[derive(Debug)]
pub(super) struct Ump {
    pub(super) set_midi_version: SetMidiVersion,
    pub(super) info_midi_version: InfoMidiVersion,
    pub(super) event_input: EventInput,
    /// Only a test's MIDI 2.0 device sends UMP.
    #[cfg(test)]
    pub(super) event_output_direct: EventOutput,
}

/// Returns the UMP calls, if this alsa-lib has them.
pub(super) fn ump() -> Option<&'static Ump> {
    static UMP: OnceLock<Option<Ump>> = OnceLock::new();
    UMP.get_or_init(|| {
        // SAFETY: each name is alsa-lib's, and each function found is
        // transmuted to the type that seq.h declares for it.
        unsafe {
            Some(Ump {
                set_midi_version: std::mem::transmute::<*mut c_void, SetMidiVersion>(symbol(
                    c"snd_seq_set_client_midi_version",
                )?),
                info_midi_version: std::mem::transmute::<*mut c_void, InfoMidiVersion>(symbol(
                    c"snd_seq_client_info_get_midi_version",
                )?),
                event_input: std::mem::transmute::<*mut c_void, EventInput>(symbol(
                    c"snd_seq_ump_event_input",
                )?),
                #[cfg(test)]
                event_output_direct: std::mem::transmute::<*mut c_void, EventOutput>(symbol(
                    c"snd_seq_ump_event_output_direct",
                )?),
            })
        }
    })
    .as_ref()
}

/// Returns the function `name` from the loaded libraries, alsa-lib among
/// them.
fn symbol(name: &CStr) -> Option<*mut c_void> {
    // SAFETY: the name lives through the call, and a name that no library
    // has returns null.
    let found = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
    (!found.is_null()).then_some(found)
}
