//! The system's notice that a MIDI device came or went.

use std::ffi::c_void;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_NOTIFY_ACTION, CM_NOTIFY_EVENT_DATA, CM_NOTIFY_FILTER, CM_NOTIFY_FILTER_0,
    CM_NOTIFY_FILTER_0_0, CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE, CM_Register_Notification,
    CM_Unregister_Notification, CR_SUCCESS, HCMNOTIFICATION,
};
use windows::core::GUID;

/// The device interface classes of MIDI inputs and outputs, whose devices
/// WinMM lists. JUCE watches the same classes.
const MIDI_INPUT: GUID = GUID::from_u128(0x504b_e32c_ccf6_4d2c_b73f_6f8b_3747_e22b);
const MIDI_OUTPUT: GUID = GUID::from_u128(0x6dc2_3320_ab33_4ce4_80d4_bbb3_ebbf_2814);

/// A watch that marks the device list as changed when a MIDI device
/// interface comes or goes.
///
/// The system's callback runs on its own thread and only sets the mark.
pub(crate) struct Watch {
    registrations: Vec<HCMNOTIFICATION>,
    /// The mark, which every registration holds, released only after they
    /// are all unregistered.
    marked: *const AtomicBool,
}

impl fmt::Debug for Watch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Watch")
            .field("registrations", &self.registrations.len())
            .finish()
    }
}

impl Watch {
    /// Starts watching, setting `marked` at each notification, or returns
    /// `None` if the system accepts no registration.
    pub(crate) fn new(marked: Arc<AtomicBool>) -> Option<Watch> {
        let context = Arc::into_raw(marked);
        let mut registrations = Vec::new();
        for class in [MIDI_INPUT, MIDI_OUTPUT] {
            let filter = CM_NOTIFY_FILTER {
                cbSize: size_of::<CM_NOTIFY_FILTER>() as u32,
                Flags: 0,
                FilterType: CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
                Reserved: 0,
                u: CM_NOTIFY_FILTER_0 {
                    DeviceInterface: CM_NOTIFY_FILTER_0_0 { ClassGuid: class },
                },
            };
            let mut registration = HCMNOTIFICATION::default();
            // SAFETY: the filter and the handle are locals that the call
            // reads and writes. The context is the mark, which outlives the
            // registration because `drop` releases it only after
            // unregistering, which waits for a callback in progress.
            let registered = unsafe {
                CM_Register_Notification(
                    &raw const filter,
                    Some(context.cast::<c_void>()),
                    Some(changed),
                    &raw mut registration,
                )
            };
            if registered == CR_SUCCESS {
                registrations.push(registration);
            }
        }
        let watch = Watch {
            registrations,
            marked: context,
        };
        // A watch with no registration releases the mark when it drops.
        (!watch.registrations.is_empty()).then_some(watch)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        for &registration in &self.registrations {
            // SAFETY: a registration that this watch made, unregistered once.
            // The call waits for a callback in progress, and none comes after.
            unsafe { CM_Unregister_Notification(registration) };
        }
        // SAFETY: the mark that `new` gave up with `into_raw`, taken back
        // once, now that no registration can reach it.
        drop(unsafe { Arc::from_raw(self.marked) });
    }
}

/// Marks the device list as changed at each notification, and does nothing
/// else.
unsafe extern "system" fn changed(
    _registration: HCMNOTIFICATION,
    context: *const c_void,
    _action: CM_NOTIFY_ACTION,
    _data: *const CM_NOTIFY_EVENT_DATA,
    _size: u32,
) -> u32 {
    // SAFETY: the context is the watch's mark, alive while any registration
    // is, as `Watch::new` says.
    let marked = unsafe { &*context.cast::<AtomicBool>() };
    // Release, because the tick that sees the mark reads the list after it.
    marked.store(true, Ordering::Release);
    // ERROR_SUCCESS.
    0
}
