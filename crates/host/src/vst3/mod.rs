//! The VST3 exporter, natively in Rust.
//!
//! A plugin crate exports its [`Plugin`] with
//! [`export_vst3!`](crate::export_vst3) and builds as a `cdylib`, which a
//! bundle step puts in a `.vst3` bundle. The host sees one class, an audio
//! processor that is its own edit controller. So the host, the processor and
//! the plugin share one parameter set, listed in declaration order and
//! identified by a fixed hash of each key.
//!
//! - A controller write while the host is processing is ignored, because the
//!   host sends the same value through `process`, in order with its
//!   automation.
//! - Edits the plugin makes on the main thread reach the host after each
//!   tick as `beginEdit`, `performEdit` and `endEdit`.
//! - One timer per instance calls [`Plugin::idle`] about 60 times a second
//!   from `initialize` to release, through a message-only window on Windows
//!   and a run loop timer on macOS. Other platforms have none, so the macro
//!   refuses to build there.
//! - The plugin is built at `initialize`, and the thread that calls it
//!   becomes the instance's main thread, whichever thread created the
//!   instance. A call that breaks [`Plugin`]'s threading rule, such as a
//!   release on another thread, counts as a fault
//!   ([`boundary_faults`](crate::boundary_faults)), as a panic does.
//! - A plugin that takes MIDI gets its host's notes, note expressions and
//!   SysEx as MIDI 2.0, each note under VST3's `noteId`, and its controllers
//!   through hidden proxy parameters, one for each channel and controller,
//!   mapped through `IMidiMapping`.
//! - What a plugin that declares MIDI output sends goes back as VST3's note
//!   events and note expressions, and its controllers as MIDI 1.0.
//! - On Windows the module stays loaded until the process ends once an
//!   instance has leaked, because its timer's window outlives it, or once an
//!   editor has opened, because its toolkit may leave the host something
//!   that points into the module.
//!
//! With the `test-host` feature, `TestHost` drives a plugin through these
//! interfaces in a test.
//!
//! Open: the `vst3` crate's 3.8.0 bindings are wrong for MIDI 2.0. Its
//! `Midi2Controller` is four bytes where the SDK's is two, so
//! `IMidiMapping2` cannot pass it across COM as they declare it.

mod component;
mod events;
mod factory;
mod stream;
#[cfg(any(test, feature = "test-host"))]
mod test_host;
mod text;
mod view;

#[cfg(test)]
mod tests;

use std::ffi::c_void;

// This module shares the dependency's name, so the bindings are `::vst3`.
use ::vst3::Steinberg::IPluginFactory;
use ::vst3::{ComPtr, ComWrapper};

#[cfg(test)]
pub(crate) use test_host::TestRunLoop;
#[cfg(any(test, feature = "test-host"))]
#[cfg_attr(docsrs, doc(cfg(feature = "test-host")))]
pub use test_host::{
    AudioSide, BusEvent, HostCall, Options, ParamInfo, RESTART_LATENCY, RESTART_VALUES, Refused,
    TestHost, TestView,
};

use crate::Plugin;
use crate::boundary::guard;
use crate::layout::Width;

/// Returns the factory for `P`, as `GetPluginFactory` does: one reference,
/// which the host releases. [`export_vst3!`](crate::export_vst3) calls it.
#[doc(hidden)]
pub fn plugin_factory<P: Plugin>() -> *mut c_void {
    guard(std::ptr::null_mut(), || {
        ComWrapper::new(factory::Factory::<P>::new())
            .to_com_ptr::<IPluginFactory>()
            .map_or(std::ptr::null_mut(), |factory| {
                ComPtr::into_raw(factory).cast::<c_void>()
            })
    })
}

/// Returns whether the host may use the module, as the host loads it. It sets
/// nothing up, so it returns `true`. The entry point that
/// [`export_vst3!`](crate::export_vst3) writes for each system calls it.
#[doc(hidden)]
pub fn enter_module() -> bool {
    true
}

/// Runs as the host unloads the module. On Windows it unregisters the
/// timer's window class. The exit point that
/// [`export_vst3!`](crate::export_vst3) writes for each system calls it.
/// Returns `false` if unregistering panicked.
#[doc(hidden)]
pub fn exit_module() -> bool {
    guard(false, || {
        crate::engine::timer::unregister();
        true
    })
}

/// Panics, at compile time in [`export_vst3!`](crate::export_vst3), for a
/// layout with a bus wider than stereo, naming it: the exporter arranges
/// none, mono and stereo buses only.
///
/// # Panics
///
/// For such a layout, which is the point.
#[doc(hidden)]
pub const fn check_layouts(layouts: &[Width]) {
    let mut index = 0;
    while index < layouts.len() {
        let Width { inputs, outputs } = layouts[index];
        if inputs > 2 || outputs > 2 {
            let mut message = Message::new();
            message.text(
                "the VST3 exporter arranges main buses of up to two channels \
                 (mono or stereo), but layout ",
            );
            message.number(index);
            message.text(" has ");
            message.number(inputs);
            message.text(" in and ");
            message.number(outputs);
            message.text(" out; export the C interface for the JUCE adapter instead");
            panic!("{}", message.as_str());
        }
        index += 1;
    }
}

/// A short message built at compile time, where no `String` exists.
struct Message {
    bytes: [u8; 192],
    length: usize,
}

impl Message {
    const fn new() -> Self {
        Message {
            bytes: [0; 192],
            length: 0,
        }
    }

    /// Appends `text`, cut short at the room left.
    const fn text(&mut self, text: &str) {
        let text = text.as_bytes();
        let mut index = 0;
        while index < text.len() && self.length < self.bytes.len() {
            self.bytes[self.length] = text[index];
            self.length += 1;
            index += 1;
        }
    }

    /// Appends `number` in decimal.
    const fn number(&mut self, mut number: usize) {
        let mut digits = [0u8; 20];
        let mut count = 0;
        loop {
            digits[count] = b'0' + (number % 10) as u8;
            count += 1;
            number /= 10;
            if number == 0 {
                break;
            }
        }
        while count > 0 && self.length < self.bytes.len() {
            count -= 1;
            self.bytes[self.length] = digits[count];
            self.length += 1;
        }
    }

    const fn as_str(&self) -> &str {
        let (used, _) = self.bytes.split_at(self.length);
        match std::str::from_utf8(used) {
            Ok(text) => text,
            // Only a cut through a character, which the text above has none
            // of.
            Err(_) => "the VST3 exporter cannot arrange a layout the plugin declares",
        }
    }
}

/// Exports a [`Plugin`] as a VST3 module, with `GetPluginFactory` and the
/// platform's module entry and exit points.
///
/// Use it once, at the root of a `cdylib` crate. A binary links only one
/// plugin's exports, because two would define the same symbols, so a crate
/// that depends on two plugin crates turns off their `vst3` features.
///
/// It refuses to build for:
///
/// - A plugin that declares no layout or a bus wider than stereo.
/// - A crate built with `panic = "abort"`, under which a panic ends the host.
/// - Any platform but Windows and macOS.
///
/// # Examples
///
/// ```no_run
#[doc = include_str!("../../doc/gain.rs")]
/// # #[cfg(any(windows, target_os = "macos"))]
/// hapto_host::export_vst3!(Gain);
/// ```
#[macro_export]
#[cfg_attr(docsrs, doc(cfg(feature = "vst3")))]
macro_rules! export_vst3 {
    ($plugin:ty) => {
        const _: () = $crate::vst3::check_layouts($crate::widths::<$plugin>());

        #[cfg(not(panic = "unwind"))]
        ::std::compile_error!(
            "a VST3 module needs panic = \"unwind\": with abort, a panic ends the host"
        );

        #[cfg(not(any(windows, target_os = "macos")))]
        ::std::compile_error!(
            "the VST3 exporter builds for Windows and macOS only: elsewhere it has no \
             main-thread timer (a Linux host offers its run loop only through an editor's \
             frame), so the plugin would never tick and a host's load would never reach its \
             session; export the C interface for the JUCE adapter instead"
        );

        /// The VST3 module's factory.
        #[allow(unsafe_code, reason = "a module entry point the host finds by name")]
        #[unsafe(no_mangle)]
        pub extern "system" fn GetPluginFactory() -> *mut ::std::ffi::c_void {
            $crate::vst3::plugin_factory::<$plugin>()
        }

        /// Called by the host as it loads the module.
        #[cfg(windows)]
        #[allow(unsafe_code, reason = "a module entry point the host finds by name")]
        #[unsafe(no_mangle)]
        pub extern "system" fn InitDll() -> bool {
            $crate::vst3::enter_module()
        }

        /// Called by the host before it unloads the module.
        #[cfg(windows)]
        #[allow(unsafe_code, reason = "a module entry point the host finds by name")]
        #[unsafe(no_mangle)]
        pub extern "system" fn ExitDll() -> bool {
            $crate::vst3::exit_module()
        }

        /// Called by the host as it loads the bundle.
        #[cfg(target_os = "macos")]
        #[allow(unsafe_code, reason = "a module entry point the host finds by name")]
        #[unsafe(no_mangle)]
        pub extern "system" fn bundleEntry(_bundle: *mut ::std::ffi::c_void) -> bool {
            $crate::vst3::enter_module()
        }

        /// Called by the host before it unloads the bundle.
        #[cfg(target_os = "macos")]
        #[allow(unsafe_code, reason = "a module entry point the host finds by name")]
        #[unsafe(no_mangle)]
        pub extern "system" fn bundleExit() -> bool {
            $crate::vst3::exit_module()
        }
    };
}
