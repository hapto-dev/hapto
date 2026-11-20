use std::time::{Duration, Instant};

use super::{Report, STALLED, Watch};

/// A text for an error, for a watch that asks for one.
fn text(why: &'static str) -> impl FnOnce() -> String {
    move || why.to_owned()
}

#[test]
fn an_error_followed_by_blocks_plays_on_and_is_said_once() {
    let (mut watch, start) = (Watch::default(), Instant::now());
    assert_eq!(watch.look(start, 1, 10, text("changed")), Report::Quiet);
    assert_eq!(
        watch.look(start + STALLED, 1, 90, text("unasked")),
        Report::PlaysOn("changed".to_owned())
    );
    assert_eq!(
        watch.look(start + STALLED * 2, 1, 180, text("unasked")),
        Report::Quiet
    );
}

#[test]
fn an_error_followed_by_no_block_is_a_device_that_stopped() {
    let (mut watch, start) = (Watch::default(), Instant::now());
    assert_eq!(watch.look(start, 1, 10, text("gone")), Report::Quiet);
    assert_eq!(
        watch.look(start + STALLED, 1, 10, text("unasked")),
        Report::Stopped("gone".to_owned())
    );
}

#[test]
fn an_error_is_not_judged_before_its_second_is_up() {
    let (mut watch, start) = (Watch::default(), Instant::now());
    let early = start + STALLED - Duration::from_millis(1);
    assert_eq!(watch.look(start, 1, 10, text("gone")), Report::Quiet);
    assert_eq!(watch.look(early, 1, 10, text("unasked")), Report::Quiet);
}

#[test]
fn blocks_that_stop_with_no_error_say_nothing_however_long() {
    let (mut watch, start) = (Watch::default(), Instant::now());
    assert_eq!(watch.look(start, 0, 10, text("unasked")), Report::Quiet);
    assert_eq!(
        watch.look(start + STALLED * 60, 0, 10, text("unasked")),
        Report::Quiet
    );
    // Nor after an error the device played on from: a pause is no stop.
    assert_eq!(watch.look(start, 1, 10, text("changed")), Report::Quiet);
    let played_on = watch.look(start + STALLED, 1, 90, text("unasked"));
    assert_eq!(played_on, Report::PlaysOn("changed".to_owned()));
    assert_eq!(
        watch.look(start + STALLED * 60, 1, 90, text("unasked")),
        Report::Quiet
    );
}

#[test]
fn a_later_error_is_judged_afresh_with_its_own_text() {
    let (mut watch, start) = (Watch::default(), Instant::now());
    assert_eq!(watch.look(start, 1, 10, text("changed")), Report::Quiet);
    let played_on = watch.look(start + STALLED, 1, 90, text("unasked"));
    assert_eq!(played_on, Report::PlaysOn("changed".to_owned()));
    let later = start + STALLED * 30;
    assert_eq!(watch.look(later, 2, 500, text("gone")), Report::Quiet);
    assert_eq!(
        watch.look(later + STALLED, 2, 500, text("unasked")),
        Report::Stopped("gone".to_owned())
    );
}
