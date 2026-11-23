//! An output thread's waits: a signal from another thread ends one at
//! once, and the timer ends one no earlier than asked.

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
        // A wait with no timeout ends only when the signal is set. One set
        // before the wait began is kept for it, as an auto-reset event
        // keeps it, so either order ends it, and never before the set.
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
