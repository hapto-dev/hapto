use std::time::{Duration, Instant};

use cpal::ErrorKind;

use super::{Answer, KINDS, Reopens, SETTLE, answer, kind_at, kind_index};

#[test]
fn each_kind_of_error_either_plays_on_or_opens_the_default_again() {
    let answers = [
        (ErrorKind::DeviceBusy, Answer::Reopen),
        (ErrorKind::DeviceChanged, Answer::PlaysOn),
        (ErrorKind::DeviceNotAvailable, Answer::Reopen),
        (ErrorKind::HostUnavailable, Answer::Reopen),
        (ErrorKind::InvalidInput, Answer::Reopen),
        (ErrorKind::PermissionDenied, Answer::Reopen),
        (ErrorKind::RealtimeDenied, Answer::PlaysOn),
        (ErrorKind::ResourceExhausted, Answer::Reopen),
        (ErrorKind::StreamInvalidated, Answer::Reopen),
        (ErrorKind::UnsupportedConfig, Answer::Reopen),
        (ErrorKind::UnsupportedOperation, Answer::Reopen),
        (ErrorKind::Xrun, Answer::PlaysOn),
        (ErrorKind::BackendError, Answer::Reopen),
        (ErrorKind::Other, Answer::Reopen),
    ];
    for (kind, expected) in answers {
        assert_eq!(answer(kind), expected, "{kind:?}");
    }
    assert_eq!(answers.len(), KINDS.len(), "every kind cpal names");
}

#[test]
fn each_kind_survives_the_atomic_and_an_unknown_place_reads_as_other() {
    for kind in KINDS {
        assert_eq!(kind_at(kind_index(kind)), kind, "{kind:?}");
    }
    assert_eq!(kind_at(u8::MAX), ErrorKind::Other);
}

#[test]
fn the_default_is_opened_again_at_most_once_a_second() {
    let (mut reopens, start) = (Reopens::default(), Instant::now());
    assert!(reopens.may(start), "the first");
    let early = start + SETTLE - Duration::from_millis(1);
    assert!(!reopens.may(early), "a device that failed at once");
    assert!(reopens.may(start + SETTLE), "a second later");
}
