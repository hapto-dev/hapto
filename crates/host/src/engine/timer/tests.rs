use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use super::*;
use crate::vst3::TestRunLoop;

/// Returns a tick that counts its calls, and the count.
fn counting() -> (Tick, Arc<AtomicU32>) {
    let ticks = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&ticks);
    let tick = Box::new(move || {
        counter.fetch_add(1, Ordering::Relaxed);
    });
    (tick, ticks)
}

#[test]
#[cfg_attr(
    any(miri, not(any(windows, target_os = "macos"))),
    ignore = "no thread timer under Miri or on this system"
)]
fn a_thread_timer_ticks_through_the_threads_loop_until_dropped() {
    let (tick, ticks) = counting();
    let timer = Timer::start(tick, Source::Thread).expect("a timer");
    pump(Duration::from_millis(400));
    // About 25 ticks at about 60 a second, but a loaded CI machine may
    // manage fewer.
    let ticked = ticks.load(Ordering::Relaxed);
    assert!(ticked >= 5, "{ticked} ticks in 400 ms");
    drop(timer);
    let stopped = ticks.load(Ordering::Relaxed);
    pump(Duration::from_millis(100));
    assert_eq!(
        ticks.load(Ordering::Relaxed),
        stopped,
        "no tick after the drop"
    );
}

#[test]
#[cfg_attr(
    any(miri, not(any(windows, target_os = "macos"))),
    ignore = "no thread timer under Miri or on this system"
)]
fn a_detached_thread_timer_never_ticks_again() {
    let (tick, ticks) = counting();
    let timer = Timer::start(tick, Source::Thread).expect("a timer");
    let handle = timer.handle();
    std::thread::spawn(move || handle.detach())
        .join()
        .expect("detached from another thread");
    pump(Duration::from_millis(100));
    assert_eq!(ticks.load(Ordering::Relaxed), 0);
    drop(timer);
}

#[cfg(all(windows, not(miri)))]
#[test]
fn a_detached_timer_s_window_stops_it_at_its_next_message() {
    let timer = Timer::start(Box::new(|| {}), Source::Thread).expect("a timer");
    let Handle::Thread(handle) = timer.handle() else {
        panic!("a thread timer");
    };
    assert!(!handle.is_stopped(), "a running timer's message is due");
    std::thread::spawn(move || handle.detach())
        .join()
        .expect("detached from another thread");
    pump(Duration::from_millis(100));
    assert!(handle.is_stopped(), "none after the one that found no tick");
    drop(timer);
}

/// Returns a run loop, and the source that offers it, as a host hands it
/// over.
fn offered() -> (::vst3::ComWrapper<TestRunLoop>, Source) {
    let run_loop = TestRunLoop::new();
    let source = TestRunLoop::source(&run_loop);
    (run_loop, source)
}

#[test]
fn a_run_loop_timer_ticks_each_time_the_loop_calls_it_until_dropped() {
    let (run_loop, source) = offered();
    let (tick, ticks) = counting();
    let timer = Timer::start(tick, source).expect("a timer");
    assert_eq!(run_loop.intervals(), [run_loop::PERIOD_MS]);
    for _ in 0..3 {
        run_loop.fire();
    }
    assert_eq!(ticks.load(Ordering::Relaxed), 3);
    drop(timer);
    assert!(run_loop.intervals().is_empty(), "unregistered");
    run_loop.fire();
    assert_eq!(ticks.load(Ordering::Relaxed), 3, "no tick after the drop");
}

#[test]
fn a_detached_run_loop_timer_never_ticks_again_but_stays_registered_until_dropped() {
    let (run_loop, source) = offered();
    let (tick, ticks) = counting();
    let timer = Timer::start(tick, source).expect("a timer");
    let handle = timer.handle();
    std::thread::spawn(move || handle.detach())
        .join()
        .expect("detached from another thread");
    run_loop.fire();
    assert_eq!(ticks.load(Ordering::Relaxed), 0);
    assert!(timer.handle().is_detached());
    assert_eq!(run_loop.intervals().len(), 1, "the host still holds it");
    drop(timer);
    assert!(run_loop.intervals().is_empty());
}

#[test]
fn a_tick_that_the_run_loop_nests_inside_another_does_nothing() {
    let (run_loop, source) = offered();
    let ticks = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&ticks);
    let nested = run_loop.clone();
    let timer = Timer::start(
        Box::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
            // A host's nested loop, run from inside the tick.
            nested.fire();
        }),
        source,
    )
    .expect("a timer");
    run_loop.fire();
    run_loop.fire();
    assert_eq!(
        ticks.load(Ordering::Relaxed),
        2,
        "one tick for each outer call"
    );
    drop(timer);
}

/// A faulty host may call the timer on another thread while the main thread
/// drops it. The drop waits for that tick, so no tick runs once the instance
/// that it reaches may be freed.
#[test]
fn a_run_loop_timer_s_drop_waits_for_a_tick_on_another_thread() {
    let (run_loop, source) = offered();
    let started = Arc::new(Barrier::new(2));
    let finished = Arc::new(AtomicBool::new(false));
    let (gate, done) = (Arc::clone(&started), Arc::clone(&finished));
    let timer = Timer::start(
        Box::new(move || {
            gate.wait();
            std::thread::sleep(Duration::from_millis(50));
            done.store(true, Ordering::Release);
        }),
        source,
    )
    .expect("a timer");
    std::thread::scope(|scope| {
        scope.spawn(|| run_loop.fire());
        started.wait();
        drop(timer);
        assert!(finished.load(Ordering::Acquire), "the tick ended first");
    });
}
