//! State the host does not list, told through the handler's second
//! interface: to the handler linked when the tick tells, to none that lacks
//! the interface, and with the new state already in a save made from inside
//! the call.

use std::cell::{Cell, RefCell};

use ::vst3::Steinberg::Vst::{
    IComponent, IComponentHandler, IComponentHandler2, IComponentHandler2Trait,
    IComponentHandlerTrait, IComponentTrait, IEditController, IEditControllerTrait, ParamID,
    ParamValue,
};
use ::vst3::Steinberg::{
    FIDString, IBStream, IPluginBaseTrait, TBool, int32, kNotImplemented, kResultOk, tresult,
};
use ::vst3::{Class, ComPtr, ComWrapper};
use hapto_params::{ParamId, ProgramParams, Value};
use serde_json::Value as Json;

use super::super::component::Component;
use super::super::test_host::{Recorder, Stream};
use super::super::{HostCall, RESTART_VALUES, TestHost};
use super::{Probe, ShapedProcessor, Table};
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

fn handed<R>(f: impl FnOnce(&Handed) -> R) -> R {
    HANDED.with(|handed| f(handed.borrow().as_ref().expect("a plugin was made")))
}

/// Renames the label, which the next tick captures.
fn rename(label: &str) {
    handed(|handed| handed.label.set(label.to_owned()));
}

/// Keeps its parts and one saved value, which the host does not list.
struct Noted {
    _parts: Parts,
}

impl Plugin for Noted {
    const INFO: PluginInfo = Probe::INFO;
    type Params = Table;
    type Processor = ShapedProcessor;

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

    fn activate(&mut self, _: &ProcessSetup) -> ShapedProcessor {
        ShapedProcessor
    }
}

/// Links `handler` as the host's, or unlinks it with `None`.
fn link(host: &TestHost<Noted>, handler: Option<&ComPtr<IComponentHandler>>) {
    let (_, _, controller) = host.interfaces();
    let pointer = handler.map_or(std::ptr::null_mut(), ComPtr::as_ptr);
    // SAFETY: the component is alive while the host holds its interfaces,
    // `pointer` is null or a live `IComponentHandler` the component takes a
    // reference to, and this is the component's main thread.
    let linked = unsafe { controller.setComponentHandler(pointer) };
    assert_eq!(linked, kResultOk);
}

#[test]
fn a_change_made_while_no_handler_is_linked_is_told_to_the_next_one() {
    let host = TestHost::<Noted>::new();
    link(&host, None);
    rename("Drums");
    host.tick();
    host.tick();
    let next = ComWrapper::new(Recorder::default());
    let handler = next.to_com_ptr::<IComponentHandler>().expect("a handler");
    link(&host, Some(&handler));
    assert_eq!(*next.calls.borrow(), [], "at a tick, not as it is linked");
    host.tick();
    assert_eq!(*next.calls.borrow(), [HostCall::Dirty(true)]);
    host.tick();
    assert_eq!(next.calls.borrow().len(), 1, "told once");
    assert_eq!(host.calls(), [], "and never to the one before");
}

/// A host's handler with the first interface alone.
#[derive(Default)]
struct First {
    calls: RefCell<Vec<HostCall>>,
}

impl Class for First {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for First {
    unsafe fn beginEdit(&self, id: ParamID) -> tresult {
        self.calls.borrow_mut().push(HostCall::Begin(id));
        kResultOk
    }

    unsafe fn performEdit(&self, id: ParamID, normalized: ParamValue) -> tresult {
        self.calls
            .borrow_mut()
            .push(HostCall::Perform(id, normalized));
        kResultOk
    }

    unsafe fn endEdit(&self, id: ParamID) -> tresult {
        self.calls.borrow_mut().push(HostCall::End(id));
        kResultOk
    }

    unsafe fn restartComponent(&self, flags: int32) -> tresult {
        self.calls.borrow_mut().push(HostCall::Restart(flags));
        kResultOk
    }
}

#[test]
fn a_handler_without_the_second_interface_is_told_nothing_and_that_is_no_fault() {
    let host = TestHost::<Noted>::new();
    let first = ComWrapper::new(First::default());
    let handler = first.to_com_ptr::<IComponentHandler>().expect("a handler");
    link(&host, Some(&handler));
    rename("Drums");
    handed(|handed| assert!(handed.program.sync(handed.mode, 0.0)));
    host.tick();
    host.tick();
    assert_eq!(
        *first.calls.borrow(),
        [HostCall::Restart(RESTART_VALUES)],
        "what the first interface has a call for, and no more"
    );
    assert_eq!(host.faults(), 0);
}

/// A host's handler that saves the instance's state from inside the call
/// that tells it the state changed, and can drop the instance's last
/// reference from inside a call, as the tick that makes it goes on.
#[derive(Default)]
struct Saver {
    instance: RefCell<Option<ComPtr<IComponent>>>,
    /// The call it drops the instance in: `"restart"` or `"dirty"`.
    release_in: Cell<&'static str>,
    calls: RefCell<Vec<HostCall>>,
    /// The block each `setDirty` saved.
    saved: RefCell<Vec<Vec<u8>>>,
}

impl Saver {
    fn release_in(&self, call: &str) {
        if self.release_in.get() == call {
            drop(self.instance.borrow_mut().take());
        }
    }
}

impl Class for Saver {
    type Interfaces = (IComponentHandler, IComponentHandler2);
}

impl IComponentHandlerTrait for Saver {
    unsafe fn beginEdit(&self, id: ParamID) -> tresult {
        self.calls.borrow_mut().push(HostCall::Begin(id));
        kResultOk
    }

    unsafe fn performEdit(&self, id: ParamID, normalized: ParamValue) -> tresult {
        self.calls
            .borrow_mut()
            .push(HostCall::Perform(id, normalized));
        kResultOk
    }

    unsafe fn endEdit(&self, id: ParamID) -> tresult {
        self.calls.borrow_mut().push(HostCall::End(id));
        kResultOk
    }

    unsafe fn restartComponent(&self, flags: int32) -> tresult {
        self.calls.borrow_mut().push(HostCall::Restart(flags));
        self.release_in("restart");
        kResultOk
    }
}

impl IComponentHandler2Trait for Saver {
    unsafe fn setDirty(&self, state: TBool) -> tresult {
        self.calls.borrow_mut().push(HostCall::Dirty(state != 0));
        if let Some(instance) = self.instance.borrow().as_ref() {
            let stream = ComWrapper::new(Stream::default());
            let pointer = stream.as_com_ref::<IBStream>().expect("a stream").as_ptr();
            // SAFETY: the instance is alive while this handler holds its
            // reference, `pointer` is a live `IBStream` until `stream` drops,
            // after the call, and this is the instance's main thread, inside
            // the tick that tells the host.
            let saved = unsafe { instance.getState(pointer) };
            assert_eq!(saved, kResultOk, "a save from inside the call");
            self.saved.borrow_mut().push(stream.bytes.borrow().clone());
        }
        self.release_in("dirty");
        kResultOk
    }

    unsafe fn requestOpenEditor(&self, _: FIDString) -> tresult {
        kNotImplemented
    }

    unsafe fn startGroupEdit(&self) -> tresult {
        kNotImplemented
    }

    unsafe fn finishGroupEdit(&self) -> tresult {
        kNotImplemented
    }
}

/// The label each block `saver` saved holds.
fn saved_labels(saver: &Saver) -> Vec<Json> {
    saver
        .saved
        .borrow()
        .iter()
        .map(|block| {
            let block: Json = serde_json::from_slice(block).expect("JSON");
            block["extra"]["label"].clone()
        })
        .collect()
}

#[test]
fn a_host_that_saves_as_it_is_told_saves_the_new_state() {
    let host = TestHost::<Noted>::new();
    let saver = ComWrapper::new(Saver::default());
    let handler = saver.to_com_ptr::<IComponentHandler>().expect("a handler");
    link(&host, Some(&handler));
    *saver.instance.borrow_mut() = Some(host.interfaces().0.clone());

    rename("Drums");
    host.tick();
    assert_eq!(*saver.calls.borrow(), [HostCall::Dirty(true)]);
    assert_eq!(saved_labels(&saver), ["Drums"]);
    rename("Bass");
    host.tick();
    assert_eq!(saved_labels(&saver), ["Drums", "Bass"]);
    assert_eq!(host.faults(), 0, "a save from inside the tick is no fault");
    // The handler's reference to the instance goes before the host's.
    saver.instance.borrow_mut().take();
}

/// Ticks an instance that has a restart and a change of unlisted state to
/// tell, with a handler that drops the instance's last reference in
/// `release_in`, and answers what the handler heard and saved.
fn released_in(release_in: &'static str) -> (Vec<HostCall>, Vec<Json>) {
    let faults = thread_faults();
    let component = Component::<Noted>::wrap(false);
    let plugin = component.to_com_ptr::<IComponent>().expect("a component");
    let controller = component
        .to_com_ptr::<IEditController>()
        .expect("its own controller");
    let saver = ComWrapper::new(Saver::default());
    saver.release_in.set(release_in);
    let handler = saver
        .as_com_ref::<IComponentHandler>()
        .expect("a handler")
        .as_ptr();
    // SAFETY: the component is alive while `plugin` and `controller` hold
    // their references, `initialize` takes a null context, `handler` is a
    // live `IComponentHandler` the component takes a reference to, and this
    // is the component's main thread.
    unsafe {
        assert_eq!(plugin.initialize(std::ptr::null_mut()), kResultOk);
        assert_eq!(controller.setComponentHandler(handler), kResultOk);
    }
    let tick = component.ticker();
    drop((component, controller));
    *saver.instance.borrow_mut() = Some(plugin);

    rename("Drums");
    handed(|handed| assert!(handed.program.sync(handed.mode, 0.0)));
    tick();
    assert!(saver.instance.borrow().is_none(), "released inside");
    let heard = saver.calls.borrow().clone();
    tick();
    assert_eq!(*saver.calls.borrow(), heard, "and ticks no more");
    assert_eq!(thread_faults(), faults);
    (heard, saved_labels(&saver))
}

#[test]
fn a_release_inside_the_restart_ends_the_ticks_calls_to_the_host() {
    let (heard, saved) = released_in("restart");
    assert_eq!(
        heard,
        [HostCall::Restart(RESTART_VALUES)],
        "the tick did not go on to tell a host that let the instance go"
    );
    assert_eq!(saved, [] as [Json; 0]);
}

#[test]
fn a_release_inside_the_telling_leaks_what_the_tick_still_uses() {
    let (heard, saved) = released_in("dirty");
    assert_eq!(
        heard,
        [HostCall::Restart(RESTART_VALUES), HostCall::Dirty(true)]
    );
    assert_eq!(saved, ["Drums"], "saved before it let go");
}
