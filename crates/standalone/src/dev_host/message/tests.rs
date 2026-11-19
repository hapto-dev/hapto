//! The settings window's messages are read from the JSON the page posts, and
//! what it shows reaches the page as one call of its `update`.

use std::time::{Duration, Instant};

use super::{About, AudioChange, AudioShown, BarShown, Message, Shown};
use crate::app::EditorShown;
use crate::dev_host::activity::Activity;
use crate::dev_host::format::{Call, Format};
use crate::dev_host::hosted::Param;
use crate::dev_host::saved::FirstLoad;
use crate::settings::{Blocks, Settings};
use crate::signal::Signal;

/// The JSON a script hands to the page's `update`.
fn handed(script: &str) -> serde_json::Value {
    let json = script
        .strip_prefix("window.update(")
        .and_then(|s| s.strip_suffix(')'))
        .expect("one call");
    serde_json::from_str(json).expect("JSON")
}

#[test]
fn a_set_and_a_switch_are_read() {
    assert_eq!(
        Message::read(r#"{"kind":"set","index":2,"value":0.25}"#),
        Some(Message::Set(2, 0.25))
    );
    assert_eq!(
        Message::read(r#"{"kind":"format","name":"cabi"}"#),
        Some(Message::Switch(Format::Cabi))
    );
}

#[test]
fn one_audio_setting_is_read_alone_and_changes_only_itself() {
    let now = Settings {
        rate: 96_000,
        max: 512,
        blocks: Blocks::Device,
    };
    let change = |text: &str| match Message::read(text) {
        Some(Message::Audio(change)) => change.apply(now),
        other => panic!("{text}: {other:?}"),
    };
    let rate = change(r#"{"kind":"audio","rate":48000}"#);
    assert_eq!(
        rate,
        Settings {
            rate: 48_000,
            ..now
        }
    );
    let max = change(r#"{"kind":"audio","max":256}"#);
    assert_eq!(max, Settings { max: 256, ..now });
    let blocks = change(r#"{"kind":"audio","blocks":"random"}"#);
    assert_eq!(
        blocks,
        Settings {
            blocks: Blocks::Random,
            ..now
        }
    );
    assert_eq!(Message::read(r#"{"kind":"audio"}"#), None, "no change");
}

#[test]
fn audio_settings_a_signal_and_processing_are_read() {
    assert_eq!(
        Message::read(r#"{"kind":"audio","rate":96000,"max":256,"blocks":"awkward"}"#),
        Some(Message::Audio(AudioChange {
            rate: Some(96_000),
            max: Some(256),
            blocks: Some(Blocks::Awkward),
        }))
    );
    assert_eq!(
        Message::read(r#"{"kind":"signal","name":"noise"}"#),
        Some(Message::Signal(Signal::Noise))
    );
    assert_eq!(
        Message::read(r#"{"kind":"reactivate"}"#),
        Some(Message::Reactivate)
    );
    assert_eq!(
        Message::read(r#"{"kind":"processing","on":false}"#),
        Some(Message::Processing(false))
    );
    assert_eq!(
        Message::read(r#"{"kind":"settings"}"#),
        Some(Message::Settings)
    );
    for text in [
        r#"{"kind":"audio","rate":-1,"max":256,"blocks":"fixed"}"#,
        r#"{"kind":"audio","max":"512"}"#,
        r#"{"kind":"audio","rate":96000,"max":256,"blocks":"jumbled"}"#,
        r#"{"kind":"signal","name":"pink"}"#,
        r#"{"kind":"processing","on":"yes"}"#,
    ] {
        assert_eq!(Message::read(text), None, "{text}");
    }
}

#[test]
fn a_page_says_when_it_is_ready() {
    assert_eq!(Message::read(r#"{"kind":"ready"}"#), Some(Message::Ready));
}

#[test]
fn anything_else_is_passed_over() {
    for text in [
        "not json",
        r#"{"kind":"set","index":-1,"value":0.5}"#,
        r#"{"kind":"set","index":0}"#,
        r#"{"kind":"format","name":"clap"}"#,
        r#"{"kind":"quit"}"#,
        "[]",
    ] {
        assert_eq!(Message::read(text), None, "{text}");
    }
}

#[test]
fn what_is_shown_reaches_the_page_as_one_update() {
    let start = Instant::now();
    let mut activity = Activity::new(start);
    let perform = Call::edit("performEdit", Some("Gain"), Some(0.5));
    activity.calls(vec![perform], start + Duration::from_millis(2));
    activity.note("activated at 48 kHz, 512 frames".into(), start);
    let params = [Param {
        id: 7,
        host_id: 7,
        handle: "id 0x00000007".into(),
        title: "Gain".into(),
        units: "dB".into(),
        steps: 0,
        kind: "continuous, dB".into(),
    }];
    let learned = ["group 1 · ch 1 · CC 74".to_owned()];
    let refused = FirstLoad::Refused {
        reason: "VST3 setState: the plugin answered kResultFalse".into(),
        block: br#"{"parameters":{}}"#.to_vec(),
    };
    let shown = Shown {
        plugin: About {
            name: "Gain \"quoted\"",
            id: "dev.test.gain",
            version: "0.1.0",
            midi_in: true,
            midi_out: false,
        },
        format: Format::Vst3,
        params: &params,
        values: vec![(0.5, "0.0".into())],
        learned: Some(&learned),
        activity: activity.since(0).collect(),
        faults: 0,
        latency: 64,
        saved: None,
        first_load: Some(&refused),
        started: 1_700_000_000_000,
        audio: AudioShown {
            device: "Speakers",
            device_rate: 48_000,
            channels: 2,
            rates: &[44_100, 48_000],
            settings: Settings::starting(48_000),
            signal: Signal::Sine,
            processing: true,
            played: 12,
            recent: vec![480, 0, 513],
            note: Some("refused"),
        },
    };
    let state = handed(&shown.script());
    let later = Shown {
        first_load: None,
        ..shown.clone()
    };
    assert!(handed(&later.script())["load"].is_null(), "sent once");
    assert_eq!(state["plugin"]["name"], "Gain \"quoted\"");
    assert_eq!(state["plugin"]["version"], "0.1.0");
    assert_eq!(state["plugin"]["midiIn"], true);
    assert_eq!(state["format"], "vst3");
    assert_eq!(state["formats"][1]["label"], "C interface");
    assert_eq!(state["params"][0]["kind"], "continuous, dB");
    assert_eq!(state["params"][0]["handle"], "id 0x00000007");
    assert_eq!(state["values"][0][1], "0.0");
    assert_eq!(state["learned"][0], "group 1 · ch 1 · CC 74");
    assert_eq!(state["activity"][0]["name"], "performEdit");
    assert_eq!(state["activity"][0]["at"], 2_000, "in microseconds");
    assert_eq!(state["activity"][0]["fields"][1][1], "0.5000");
    assert_eq!(state["activity"][1]["kind"], "note");
    assert_eq!(state["latency"], 64);
    assert!(state["saved"].is_null());
    assert_eq!(state["load"]["kind"], "refused");
    assert!(
        state["load"]["block"]
            .as_str()
            .expect("text")
            .contains("parameters")
    );
    assert_eq!(state["started"], 1_700_000_000_000_u64);
    let audio = &state["audio"];
    assert_eq!(audio["rates"][0]["label"], "44.1 kHz");
    assert_eq!(audio["blocks"], "device");
    assert_eq!(audio["plans"][3]["label"], "Awkward");
    assert_eq!(audio["signal"], "sine");
    assert_eq!(audio["signals"][0]["name"], "silence");
    assert_eq!(audio["processing"], true);
    assert_eq!(audio["recent"][2], 513);
    assert_eq!(audio["note"], "refused");
}

#[test]
fn the_bar_shows_the_format_the_audio_and_the_faults() {
    let mut bar = BarShown {
        format: Format::Vst3,
        settings: Settings {
            rate: 44_100,
            max: 512,
            blocks: Blocks::Device,
        },
        processing: true,
        faults: 0,
        editor: EditorShown::Open,
    };
    assert_eq!(bar.status(), "44.1 kHz, up to 512 frames, no faults");
    bar.settings.rate = 96_000;
    bar.faults = 2;
    assert_eq!(bar.status(), "96 kHz, up to 512 frames, 2 faults");
    bar.processing = false;
    assert_eq!(bar.status(), "stopped, 2 faults");
    let shown = handed(&bar.script());
    assert_eq!(shown["format"], "VST3");
    assert!(shown["note"].is_null(), "the editor is below");
    bar.editor = EditorShown::None;
    let shown = handed(&bar.script());
    let note = shown["note"].as_str().expect("a note");
    assert!(
        note.starts_with("The plugin has no editor"),
        "VST3 has views; the plugin offers none: {note}"
    );
    bar.format = Format::Cabi;
    let shown = handed(&bar.script());
    assert!(
        shown["note"]
            .as_str()
            .expect("a note")
            .starts_with("C interface has no editor")
    );
    bar.editor = EditorShown::Failed;
    let shown = handed(&bar.script());
    let note = shown["note"].as_str().expect("a note");
    assert!(
        note.starts_with("The plugin's editor did not open"),
        "not taken for one it lacks: {note}"
    );
}
