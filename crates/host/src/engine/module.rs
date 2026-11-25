//! The plugin's own module, the library that the host loaded the plugin from.
//!
//! Once any editor has opened, or a timer has leaked, the module stays loaded
//! until the process ends. Only a Windows VST3 module needs the pin today.
//! macOS opens no editor yet, Linux builds no VST3 module yet, and a test
//! binary or an app is never unloaded.
//!
//! Open: a Linux VST3 module needs the pin too, for what an editor's toolkit
//! leaves and for a detached run loop timer's handler that the host still
//! holds.

#[cfg(all(windows, feature = "vst3", not(miri)))]
use windows_sys::Win32::Foundation::HINSTANCE;

/// Returns this module's handle. Window classes belong to the module that
/// registers them, so two plugins in one host each register their own.
#[cfg(all(windows, feature = "vst3", not(miri)))]
pub(crate) fn handle() -> HINSTANCE {
    use windows_sys::Win32::System::LibraryLoader::GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;

    find(GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT)
}

/// Keeps this module loaded until the process ends, from any thread. The
/// first call pins it, and every later call changes nothing.
///
/// The pin is for what the module may have left with the host, such as an
/// detached timer's window, or a window class, subclass or hook that an
/// editor's toolkit made. A host may unload the module once its last
/// instance is gone and load it again at another address, and the system
/// would then call code that is no longer mapped. A pinned module keeps its
/// mapping, so after `ExitDll` the host's next load gets the same one, and
/// `InitDll` runs again with every static as it was left. No static in Hapto
/// assumes a fresh mapping, and the timer's window class is unregistered at
/// `ExitDll` and registered again by the next timer.
///
/// The cost is that the module's file stays in use until the host exits. The
/// pin can't be tested in-process, because a test binary is never unloaded.
#[cfg(all(windows, feature = "vst3", not(miri)))]
pub(crate) fn keep_loaded() {
    use windows_sys::Win32::System::LibraryLoader::GET_MODULE_HANDLE_EX_FLAG_PIN;

    // The result goes unchecked, because the address is one of the running
    // module's own functions, which the loader always finds.
    find(GET_MODULE_HANDLE_EX_FLAG_PIN);
}

/// Does nothing, because nothing here needs the module pinned.
#[cfg(not(all(windows, feature = "vst3", not(miri))))]
pub(crate) fn keep_loaded() {}

/// Returns this module's handle, found from the address of one of its own
/// functions, with `flags` added.
#[cfg(all(windows, feature = "vst3", not(miri)))]
fn find(flags: u32) -> HINSTANCE {
    use std::ffi::c_void;

    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GetModuleHandleExW,
    };

    let mut module: HINSTANCE = std::ptr::null_mut();
    // SAFETY: the address is this module's own function, `flags` either adds
    // no reference or pins the module, and `module` has room for the handle.
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | flags,
            find as *const c_void as *const u16,
            &mut module,
        )
    };
    module
}
