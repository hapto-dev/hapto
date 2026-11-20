use std::time::Duration;

use super::{STALLED, stopped};

#[test]
fn an_error_alone_does_not_stop_a_device_that_still_plays() {
    assert!(!stopped(true, Duration::ZERO));
    assert!(!stopped(true, STALLED - Duration::from_millis(1)));
}

#[test]
fn an_error_and_no_block_for_a_second_is_a_device_that_stopped() {
    assert!(stopped(true, STALLED));
}

#[test]
fn a_device_that_reported_nothing_has_not_stopped_however_quiet() {
    assert!(!stopped(false, STALLED * 10));
}
