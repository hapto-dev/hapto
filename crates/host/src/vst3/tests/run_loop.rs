//! Tests the engine's timer on a host's run loop, through a component: a timer
//! that runs until release, a second start, a tick on the wrong thread, a
//! release inside a tick or on another thread, and a run loop that refuses
//! the timer.

use ::vst3::ComWrapper;
use ::vst3::Steinberg::Vst::{
    IComponent, IComponentHandler, IComponentTrait, IEditController, IEditControllerTrait,
};
use ::vst3::Steinberg::kResultOk;

use super::super::TestRunLoop;
use super::super::component::Component;
use super::{Probe, Releaser, initialized, probe};
use crate::faults::thread_faults;

/// Returns a component, initialized on this thread, whose timer runs on a
/// run loop that the test fires, and the run loop.
fn on_a_run_loop() -> (ComWrapper<Component<Probe>>, ComWrapper<TestRunLoop>) {
    let component = initialized();
    let run_loop = TestRunLoop::new();
    component.start_timer(TestRunLoop::source(&run_loop));
    (component, run_loop)
}

#[test]
fn a_run_loop_timer_ticks_the_plugin_until_the_instance_is_released() {
    let (component, run_loop) = on_a_run_loop();
    let idles = probe(|state| state.idles);
    run_loop.fire();
    run_loop.fire();
    assert_eq!(probe(|state| state.idles), idles + 2);
    drop(component);
    assert!(
        run_loop.intervals().is_empty(),
        "the release unregistered it"
    );
}

#[test]
fn a_timer_start_while_one_runs_starts_none() {
    let (component, run_loop) = on_a_run_loop();
    let other = TestRunLoop::new();
    component.start_timer(TestRunLoop::source(&other));
    assert!(other.intervals().is_empty(), "the second run loop has none");
    assert_eq!(run_loop.intervals().len(), 1, "the first timer runs on");
}

#[test]
fn a_tick_on_the_wrong_thread_counts_one_fault_and_detaches_the_timer() {
    let (component, run_loop) = on_a_run_loop();
    let faults = component.faults();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for _ in 0..3 {
                run_loop.fire();
            }
        });
    });
    assert_eq!(component.faults(), faults + 1, "counted once");
    let idles = probe(|state| state.idles);
    run_loop.fire();
    assert_eq!(
        probe(|state| state.idles),
        idles,
        "detached on this thread too"
    );
    drop(component);
    assert!(
        run_loop.intervals().is_empty(),
        "the release still unregistered it"
    );
}

/// A release from a host callback inside a tick leaks the engine, timer
/// included, and must detach the timer from its tick.
#[test]
fn a_release_inside_a_run_loop_tick_detaches_the_timer() {
    let (component, run_loop) = on_a_run_loop();
    let handle = component.timer_handle().expect("a timer");
    let plugin = component.to_com_ptr::<IComponent>().expect("a component");
    let controller = component
        .to_com_ptr::<IEditController>()
        .expect("its own controller");
    drop(component);
    let releaser = ComWrapper::new(Releaser::default());
    let handler = releaser
        .as_com_ref::<IComponentHandler>()
        .expect("a handler")
        .as_ptr();
    // SAFETY: the component is alive while `plugin` and `controller` hold
    // their references, `handler` is a live `IComponentHandler` that the
    // component takes a reference to, and this is the component's main
    // thread.
    unsafe {
        assert_eq!(controller.setComponentHandler(handler), kResultOk);
        assert_eq!(plugin.setActive(1), kResultOk);
    }
    drop(controller);
    *releaser.instance.borrow_mut() = Some(plugin);
    probe(|state| state.edit = Some(("gain", 6.0)));
    run_loop.fire();
    assert!(
        releaser.instance.borrow().is_none(),
        "released inside the tick"
    );
    assert!(handle.is_detached(), "the leaked timer never ticks again");
    let idles = probe(|state| state.idles);
    run_loop.fire();
    assert_eq!(probe(|state| state.idles), idles);
    assert_eq!(probe(|state| state.drops), 0, "the plugin leaked");
}

/// A release on another thread leaks the engine, timer included, and must
/// detach the timer from its tick from there.
#[test]
fn a_release_on_another_thread_detaches_the_run_loop_timer() {
    let (component, run_loop) = on_a_run_loop();
    let handle = component.timer_handle().expect("a timer");
    let plugin = component.to_com_ptr::<IComponent>().expect("a component");
    drop(component);
    let faults = std::thread::spawn(move || {
        let faults = thread_faults();
        drop(plugin);
        thread_faults() - faults
    })
    .join()
    .expect("released there");
    assert_eq!(faults, 1);
    assert!(handle.is_detached(), "the leaked timer never ticks again");
    let idles = probe(|state| state.idles);
    run_loop.fire();
    assert_eq!(probe(|state| state.idles), idles);
    assert_eq!(probe(|state| state.drops), 0, "the plugin leaked");
}

/// A faulty host may refuse the timer but keep its handler and call it later.
#[test]
fn a_run_loop_that_refuses_the_timer_counts_a_fault_and_never_ticks_it() {
    let component = initialized();
    let run_loop = TestRunLoop::refusing();
    let faults = component.faults();
    component.start_timer(TestRunLoop::source(&run_loop));
    assert_eq!(component.faults(), faults + 1, "the refusal counted");
    assert!(component.timer_handle().is_none());
    let idles = probe(|state| state.idles);
    run_loop.fire();
    assert_eq!(
        probe(|state| state.idles),
        idles,
        "the kept handler reaches no tick"
    );
}
