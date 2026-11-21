use cpal::ErrorKind;

use super::{Answer, KINDS, Look, answer, kind_at, kind_index, look, say_once, text};

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
fn a_device_that_went_away_says_so_and_every_other_kind_keeps_cpal_s_text() {
    let gone = || {
        cpal::Error::with_message(
            ErrorKind::DeviceNotAvailable,
            "Failed to get current padding: OS Error -2004287484",
        )
    };
    let plain = "the audio device went away";
    assert_eq!(text(ErrorKind::DeviceNotAvailable, Some(gone())), plain);
    assert_eq!(text(ErrorKind::DeviceNotAvailable, None), plain, "lost");
    let busy = cpal::Error::with_message(ErrorKind::DeviceBusy, "in use");
    assert_eq!(text(ErrorKind::DeviceBusy, Some(busy)), "in use");
    assert_eq!(
        text(ErrorKind::StreamInvalidated, Some(gone())),
        ErrorKind::StreamInvalidated.to_string(),
        "an older error's text is not the newest's"
    );
}

#[test]
fn a_look_keeps_moves_stops_opens_or_waits_by_the_two_ids() {
    let (one, two) = (1, 2);
    assert_eq!(look(Some(&one), Some(&one)), Look::Keep, "the same device");
    assert_eq!(look(Some(&one), Some(&two)), Look::Move, "another device");
    assert_eq!(look(Some(&one), None), Look::Stop, "none");
    assert_eq!(look(None, Some(&two)), Look::Open, "nothing plays");
    assert_eq!(look::<u32>(None, None), Look::Wait, "nothing to open");
}

#[test]
fn a_failure_repeated_at_every_look_is_said_once_and_again_when_it_changes() {
    let mut said = None;
    let busy = || "the device is in use".to_owned();
    assert_eq!(say_once(&mut said, busy()), Some(busy()), "the first");
    assert_eq!(say_once(&mut said, busy()), None, "the same again");
    let gone = "there is no audio output device".to_owned();
    assert_eq!(say_once(&mut said, gone.clone()), Some(gone), "a new one");
    said = None;
    assert_eq!(say_once(&mut said, busy()), Some(busy()), "after a start");
}
