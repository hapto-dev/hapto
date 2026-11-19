//! State the host does not list, told through the restart callback: with
//! its own flag, in a call after the other flags', with the new state
//! already in a save made from inside the call, and never for a load the
//! host makes from inside the call before it.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use hapto_params::{ParamId, ProgramParams, Value};
use serde_json::Value as Json;

use super::super::{HaptoHost, HaptoInstance, RESTART_STATE, RESTART_VALUES, header, plugin};
use super::{Probe, Table, Through};
use crate::faults::thread_faults;
use crate::{Parts, Plugin, PluginInfo, ProcessSetup};

/// What [`Noted`] hands the test, which runs it on the test's own thread.
struct Handed {
    /// A value saved under `label`.
    label: Value<String>,
    program: ProgramParams,
    mode: ParamId,
}

thread_local! {
    static HANDED: RefCell<Option<Handed>> = const { RefCell::new(None) };
}

/// Keeps its parts and one saved value, which the host does not list.
struct Noted {
    _parts: Parts,
}

impl Plugin for Noted {
    const INFO: PluginInfo = Probe::INFO;
    type Params = Table;
    type Processor = Through;

    fn new(_: Table, mut parts: Parts) -> Self {
        let label = parts
            .saved_value("label", String::from("Untitled"))
            .expect("a free key");
        HANDED.set(Some(Handed {
            label,
            program: parts.set().program(),
            mode: parts.set().id("mode").expect("declared"),
        }));
        Noted { _parts: parts }
    }

    fn activate(&mut self, _: &ProcessSetup) -> Through {
        Through
    }
}

/// What [`Saving`] does from inside its first restart call.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Inside {
    Nothing,
    /// Destroys the instance.
    Destroy,
    /// Loads the block the instance saved as it was made.
    Load,
}

/// A host whose restart callback records its flags, saves the instance's
/// state when they say it changed, and acts as [`Inside`] says from inside
/// its first call. It asserts nothing there: a panic in an `extern "C"`
/// callback aborts the test binary with no failure named, so the test's
/// body checks what it recorded.
struct Saving {
    instance: Cell<*mut HaptoInstance>,
    inside: Inside,
    /// What the instance saved as it was made.
    made: RefCell<Vec<u8>>,
    restarts: RefCell<Vec<u32>>,
    /// The block each call with the state flag saved, or `None` where the
    /// save was refused.
    saved: RefCell<Vec<Option<Vec<u8>>>>,
    /// Whether the load from inside the first call took.
    loaded: Cell<Option<bool>>,
}

impl Saving {
    fn new(inside: Inside) -> Box<Saving> {
        Box::new(Saving {
            instance: Cell::new(std::ptr::null_mut()),
            inside,
            made: RefCell::new(Vec::new()),
            restarts: RefCell::new(Vec::new()),
            saved: RefCell::new(Vec::new()),
            loaded: Cell::new(None),
        })
    }

    /// The instance's saved block, or `None` when the save was refused.
    ///
    /// # Safety
    ///
    /// The instance is live, and this runs on its main thread.
    unsafe fn save(&self) -> Option<Vec<u8>> {
        let mut block: Vec<u8> = Vec::new();
        // SAFETY: the caller's contract. `collect` reads its context as the
        // `Vec<u8>` it is, which outlives the call, and never unwinds.
        let saved = unsafe {
            (plugin::<Noted>().save)(
                self.instance.get(),
                (&raw mut block).cast::<c_void>(),
                Some(collect),
            )
        };
        saved.then_some(block)
    }
}

unsafe extern "C" fn saving_restart(context: *mut c_void, flags: u32) {
    // SAFETY: the context is the test's `Saving`, which outlives the
    // instance and is shared only through `&` references.
    let host = unsafe { &*context.cast::<Saving>() };
    let first = host.restarts.borrow().is_empty();
    host.restarts.borrow_mut().push(flags);
    if flags & RESTART_STATE != 0 {
        // SAFETY: the live instance, on its main thread, from inside its own
        // tick.
        let saved = unsafe { host.save() };
        host.saved.borrow_mut().push(saved);
    }
    if !first {
        return;
    }
    let table = plugin::<Noted>();
    match host.inside {
        Inside::Nothing => {}
        Inside::Destroy => {
            let instance = host.instance.replace(std::ptr::null_mut());
            if !instance.is_null() {
                // SAFETY: the instance this test made, destroyed on its main
                // thread from inside its own tick.
                unsafe { (table.destroy)(instance) };
            }
        }
        Inside::Load => {
            let made = host.made.borrow();
            // SAFETY: the live instance, on its main thread, from inside its
            // own tick; `made` is valid for reading its length during the
            // call.
            let loaded = unsafe { (table.load)(host.instance.get(), made.as_ptr(), made.len()) };
            host.loaded.set(Some(loaded));
        }
    }
}

/// `save`'s writer, appending to the `Vec` behind `context`.
unsafe extern "C" fn collect(context: *mut c_void, data: *const u8, length: usize) {
    // SAFETY: `context` is the `Vec<u8>` the save was passed, borrowed by
    // nothing else during the call, and the header makes `data` valid for
    // reading `length` bytes until this returns.
    unsafe {
        let bytes = &mut *context.cast::<Vec<u8>>();
        if length > 0 {
            bytes.extend_from_slice(std::slice::from_raw_parts(data, length));
        }
    }
}

/// Creates the plugin with `host`'s callback and keeps what it saves as it
/// is made, renames its label and syncs a listed parameter, and ticks it
/// twice.
fn tick_twice_with(host: &Saving) {
    let table = plugin::<Noted>();
    let callbacks = HaptoHost {
        context: std::ptr::from_ref::<Saving>(host)
            .cast_mut()
            .cast::<c_void>(),
        restart: Some(saving_restart),
        ..HaptoHost::NONE
    };
    // SAFETY: `callbacks` is readable for `create`, its callback never
    // unwinds, and its context, the test's host, outlives the instance;
    // every later call is on the instance `create` answered, on its main
    // thread, unless the callback destroyed it and left a null.
    unsafe {
        host.instance.set((table.create)(&callbacks));
        *host.made.borrow_mut() = host.save().expect("saved as it was made");
        HANDED.with(|handed| {
            let handed = handed.borrow();
            let handed = handed.as_ref().expect("a plugin was made");
            handed.label.set(String::from("Drums"));
            assert!(handed.program.sync(handed.mode, 0.0));
        });
        for _ in 0..2 {
            let instance = host.instance.get();
            if !instance.is_null() {
                (table.idle)(instance);
            }
        }
    }
}

/// Destroys `host`'s instance, if it is still live.
fn destroy(host: &Saving) {
    let instance = host.instance.replace(std::ptr::null_mut());
    if !instance.is_null() {
        // SAFETY: the live instance this test made, on its main thread.
        unsafe { (plugin::<Noted>().destroy)(instance) };
    }
}

#[test]
fn the_state_flag_comes_alone_after_the_others_and_a_save_inside_holds_the_new_state() {
    let faults = thread_faults();
    let host = Saving::new(Inside::Nothing);
    tick_twice_with(&host);
    assert_eq!(
        *host.restarts.borrow(),
        [RESTART_VALUES, RESTART_STATE],
        "each told once, in two calls"
    );
    let saved = host.saved.borrow();
    let block = saved[0].as_ref().expect("a save from inside the call");
    let block: Json = serde_json::from_slice(block).expect("JSON");
    assert_eq!(block["extra"]["label"], "Drums");
    destroy(&host);
    assert_eq!(thread_faults(), faults);
}

#[test]
fn a_destroy_inside_the_restart_ends_the_ticks_calls_to_the_host() {
    let faults = thread_faults();
    let host = Saving::new(Inside::Destroy);
    tick_twice_with(&host);
    assert!(host.instance.get().is_null(), "destroyed in the tick");
    assert_eq!(
        *host.restarts.borrow(),
        [RESTART_VALUES],
        "the state is not told to a host that may be gone"
    );
    assert_eq!(thread_faults(), faults);
}

#[test]
fn a_load_inside_the_restart_is_not_told_as_a_change() {
    let faults = thread_faults();
    let host = Saving::new(Inside::Load);
    tick_twice_with(&host);
    assert_eq!(host.loaded.get(), Some(true), "the load took");
    assert_eq!(
        *host.restarts.borrow(),
        [RESTART_VALUES],
        "the rename the load replaced is not told"
    );
    // SAFETY: the live instance this test made, on its main thread.
    let saved = unsafe { host.save() }.expect("saved");
    assert_eq!(saved, *host.made.borrow(), "and it holds what it loaded");
    destroy(&host);
    assert_eq!(thread_faults(), faults);
}

#[test]
fn the_header_defines_the_state_flag_beside_the_other_two() {
    let text = header();
    for define in [
        "#define HAPTO_RESTART_VALUES 1u",
        "#define HAPTO_RESTART_LATENCY 2u",
        "#define HAPTO_RESTART_STATE 4u",
    ] {
        assert!(text.contains(define), "{define}");
    }
}
