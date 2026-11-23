//! The app's MIDI routing with no device: filters set and changed, and what
//! a change opens and closes.

use hapto_midi_devices::Direction;

use super::*;

fn keys() -> Device {
    Device::for_tests("Keys", Direction::Input)
}

fn synth() -> Device {
    Device::for_tests("Synth", Direction::Output)
}

#[test]
fn every_input_and_no_output_by_default_and_each_change_is_counted() {
    let midi = MidiDevices::new();
    let (changes, inputs, outputs) = midi.filters();
    assert_eq!(changes, 0);
    assert!(inputs(&keys()), "every input");
    assert!(!outputs(&synth()), "no output");
    midi.set_outputs(|_| true);
    midi.set_inputs(|_| false);
    assert_eq!(midi.timing(), hapto_midi_devices::Timing::Immediate);
    midi.set_timing(hapto_midi_devices::Timing::AtFrames);
    let (changes, _, _) = midi.filters();
    assert_eq!(changes, 3, "three changes to apply");
    assert_eq!(midi.timing(), hapto_midi_devices::Timing::AtFrames);
    assert!(midi.listed().is_empty() && midi.open().is_empty());
}

#[test]
fn a_change_opens_what_now_passes_and_closes_what_no_longer_does() {
    // Listed with whether the filter passes each.
    let listed = [("keys", true), ("pads", false), ("synth", true)];
    let open = ["pads", "gone"];
    let (to_open, to_close) = plan(&listed, &open);
    assert_eq!(to_open, ["keys", "synth"]);
    assert_eq!(
        to_close,
        ["pads", "gone"],
        "one the filter refuses, one the system no longer lists"
    );
    let (to_open, to_close) = plan(&listed, &["keys", "synth"]);
    assert!(to_open.is_empty() && to_close.is_empty(), "nothing to do");
}

#[test]
fn a_clone_reaches_the_same_routing_from_another_thread() {
    let midi = MidiDevices::new();
    let elsewhere = midi.clone();
    std::thread::spawn(move || elsewhere.set_outputs(|_| true))
        .join()
        .expect("the other thread");
    let (changes, _, outputs) = midi.filters();
    assert_eq!(changes, 1);
    assert!(outputs(&synth()), "the other thread's filter");
    // Called with no lock held, a filter may reach the routing itself.
    let reaching = midi.clone();
    midi.set_inputs(move |_| reaching.open().is_empty());
    let (_, inputs, _) = midi.filters();
    assert!(inputs(&keys()), "no deadlock, and nothing open");
}
