//! The Activity log stamps each entry from the run's start, numbers them so
//! the page takes each once, and keeps the latest.

use std::time::{Duration, Instant};

use super::{Activity, KEPT, What};
use crate::dev_host::format::Call;

#[test]
fn entries_are_stamped_from_the_start_and_numbered_in_order() {
    let start = Instant::now();
    let mut log = Activity::new(start);
    assert_eq!(log.last(), 0);
    let begin = Call::edit("beginEdit", Some("Gain"), None);
    log.calls(vec![begin.clone()], start + Duration::from_millis(5));
    log.note("reactivated".into(), start + Duration::from_millis(9));
    let entries: Vec<_> = log.since(0).cloned().collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(
        (entries[0].number, entries[0].at),
        (1, Duration::from_millis(5))
    );
    assert_eq!(entries[0].what, What::Call(begin));
    assert_eq!(entries[1].what, What::Note("reactivated".into()));
    assert_eq!(log.since(1).count(), 1, "after the first");
    assert_eq!(log.since(log.last()).count(), 0);
}

#[test]
fn the_log_keeps_the_latest_entries() {
    let start = Instant::now();
    let mut log = Activity::new(start);
    for _ in 0..KEPT + 5 {
        log.note("blocks".into(), start);
    }
    let numbers: Vec<u64> = log.since(0).map(|entry| entry.number).collect();
    assert_eq!(numbers.len(), KEPT);
    assert_eq!(numbers.first(), Some(&6));
    assert_eq!(log.last(), u64::try_from(KEPT + 5).expect("small"));
}
