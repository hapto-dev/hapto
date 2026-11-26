//! Tests that a signal from another thread ends an output thread's wait at
//! once, and that the timer never ends one early.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use super::*;

#[test]
fn a_signal_from_another_thread_ends_a_wait_for_it() {
    let signal = Arc::new(Signal::new());
    let setter = Arc::clone(&signal);
    let set = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&set);
    let waiting = thread::spawn(move || {
        // A wait with no timeout ends only when the signal is set. An
        // auto-reset event stays set until a wait takes it, so either order
        // ends the wait, never before the set.
        Timer::new().wait(&signal, None);
        seen.load(Ordering::Acquire)
    });
    set.store(true, Ordering::Release);
    setter.set();
    let after = waiting.join().expect("the waiting thread");
    assert!(after, "the wait ended before the signal was set");
}

#[test]
fn the_timer_ends_a_wait_no_earlier_than_asked() {
    let (signal, timer) = (Signal::new(), Timer::new());
    let mut late = Vec::new();
    for _ in 0..20 {
        let started = Instant::now();
        timer.wait(&signal, Some(Duration::from_millis(3)));
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(3), "{waited:?}");
        late.push(waited - Duration::from_millis(3));
    }
    late.sort();
    println!(
        "late by {:?} at the median, {:?} at worst",
        late[10], late[19]
    );
}
