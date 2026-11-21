//! What the dev host's pages post, the bar and the settings window, and
//! what they are told.

#[cfg(test)]
mod tests;

use serde_json::{Value as Json, json};

use super::activity::{Entry, KEPT, What};
use super::audio::khz;
use super::format::Format;
use super::hosted::Param;
use super::saved::FirstLoad;
use crate::app::EditorShown;
use crate::block::preview;
use crate::settings::{Blocks, MAX_BLOCKS, Settings};
use crate::signal::Signal;

/// The plan cards' pictures: each plan's first [`PREVIEW_BLOCKS`] blocks,
/// of at most [`PREVIEW_MAX`] frames, from a device asking for
/// [`PREVIEW_BUFFER`] frames at a time.
const PREVIEW_MAX: usize = 64;
/// The device's buffer in the plan cards' pictures.
const PREVIEW_BUFFER: usize = 448;
/// The blocks in a plan card's picture.
const PREVIEW_BLOCKS: usize = 12;

/// A message one of the dev host's pages posted.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Message {
    /// Set the parameter at this index of the host's list to this 0..1.
    Set(usize, f64),
    /// Start again in this format, with the saved block.
    Switch(Format),
    /// Change these audio settings, keeping the others.
    Audio(AudioChange),
    /// Play this signal into the plugin's inputs.
    Signal(Signal),
    /// Reactivate the plugin with the settings it has.
    Reactivate,
    /// Stop processing and deactivate the plugin, or with `true`, activate
    /// it again.
    Processing(bool),
    /// Open the settings window, or bring it to the front.
    Settings,
    /// The settings window's page has loaded, and takes updates.
    Ready,
}

impl Message {
    /// Reads a message the page posted, as JSON: `{"kind": "set", "index":
    /// 0, "value": 0.5}`, `{"kind": "format", "name": "cabi"}`, `{"kind":
    /// "audio", "rate": 48000}`, with any of `rate`, `max` and `blocks`,
    /// `{"kind": "signal", "name": "sine"}`, `{"kind": "reactivate"}`,
    /// `{"kind": "processing", "on": false}`, `{"kind": "settings"}` or
    /// `{"kind": "ready"}`. `None` for anything else.
    pub(crate) fn read(text: &str) -> Option<Message> {
        let value: Json = serde_json::from_str(text).ok()?;
        match value["kind"].as_str()? {
            "set" => {
                let index = usize::try_from(value["index"].as_u64()?).ok()?;
                Some(Message::Set(index, value["value"].as_f64()?))
            }
            "format" => Format::named(value["name"].as_str()?).map(Message::Switch),
            "audio" => AudioChange::read(&value).map(Message::Audio),
            "signal" => Signal::named(value["name"].as_str()?).map(Message::Signal),
            "reactivate" => Some(Message::Reactivate),
            "processing" => Some(Message::Processing(value["on"].as_bool()?)),
            "settings" => Some(Message::Settings),
            "ready" => Some(Message::Ready),
            _ => None,
        }
    }
}

/// A change of the audio settings, as the page posts one: only what the
/// person changed, so a change sent while the page shows settings a moment
/// old does not send those back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AudioChange {
    /// The new sample rate.
    pub(crate) rate: Option<u32>,
    /// The new most frames a block holds.
    pub(crate) max: Option<usize>,
    /// The new cut.
    pub(crate) blocks: Option<Blocks>,
}

impl AudioChange {
    /// Reads the fields of a message there are, each of them valid; `None`
    /// for one with none, or with one that is not.
    fn read(value: &Json) -> Option<AudioChange> {
        let field = |name: &str| (!value[name].is_null()).then(|| &value[name]);
        let change = AudioChange {
            rate: match field("rate") {
                Some(rate) => Some(u32::try_from(rate.as_u64()?).ok()?),
                None => None,
            },
            max: match field("max") {
                Some(max) => Some(usize::try_from(max.as_u64()?).ok()?),
                None => None,
            },
            blocks: match field("blocks") {
                Some(blocks) => Some(Blocks::named(blocks.as_str()?)?),
                None => None,
            },
        };
        (change != AudioChange::default()).then_some(change)
    }

    /// Returns `settings` with the change made.
    pub(crate) fn apply(self, settings: Settings) -> Settings {
        Settings {
            rate: self.rate.unwrap_or(settings.rate),
            max: self.max.unwrap_or(settings.max),
            blocks: self.blocks.unwrap_or(settings.blocks),
        }
    }
}

/// The plugin as the settings window shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct About<'a> {
    /// Its name.
    pub(crate) name: &'a str,
    /// Its reverse-DNS id.
    pub(crate) id: &'a str,
    /// Its version.
    pub(crate) version: &'a str,
    /// Whether it takes MIDI in.
    pub(crate) midi_in: bool,
    /// Whether it sends MIDI out.
    pub(crate) midi_out: bool,
}

/// What the settings window shows, as the page's `update` takes it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Shown<'a> {
    /// The plugin.
    pub(crate) plugin: About<'a>,
    /// The format the plugin runs through now.
    pub(crate) format: Format,
    /// The parameters, as the host lists them.
    pub(crate) params: &'a [Param],
    /// Each parameter's 0..1 and text.
    pub(crate) values: Vec<(f64, String)>,
    /// What each parameter was learned from, when the block changed.
    pub(crate) learned: Option<&'a [String]>,
    /// The Activity log's entries the page has not had.
    pub(crate) activity: Vec<&'a Entry>,
    /// The instance's faults.
    pub(crate) faults: u64,
    /// The plugin's latency, in frames.
    pub(crate) latency: u32,
    /// The saved block, as its text, when it changed since the last update.
    pub(crate) saved: Option<String>,
    /// How the run's first load of the saved block went, in the update that
    /// brings everything: it never changes.
    pub(crate) first_load: Option<&'a FirstLoad>,
    /// When the run started, in milliseconds since 1970, for the page to
    /// show in local time.
    pub(crate) started: u64,
    /// The audio the plugin runs with.
    pub(crate) audio: AudioShown<'a>,
}

/// What the settings window shows of the audio.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AudioShown<'a> {
    /// The output device's name.
    pub(crate) device: &'a str,
    /// The device's own sample rate.
    pub(crate) device_rate: u32,
    /// How many channels the device plays.
    pub(crate) channels: usize,
    /// The sample rates the device takes, which alone are offered.
    pub(crate) rates: &'a [u32],
    /// The settings the plugin runs with.
    pub(crate) settings: Settings,
    /// The signal on the plugin's inputs.
    pub(crate) signal: Signal,
    /// Whether the plugin processes.
    pub(crate) processing: bool,
    /// How many blocks were played.
    pub(crate) played: u64,
    /// The latest blocks' lengths, the last one last.
    pub(crate) recent: Vec<u32>,
    /// Why the last change did not take.
    pub(crate) note: Option<&'a str>,
}

impl Shown<'_> {
    /// Returns the script that hands it to the page.
    pub(crate) fn script(&self) -> String {
        let state = json!({
            "plugin": {
                "name": self.plugin.name,
                "id": self.plugin.id,
                "version": self.plugin.version,
                "midiIn": self.plugin.midi_in,
                "midiOut": self.plugin.midi_out,
            },
            "format": self.format.name(),
            "formats": Format::ALL
                .iter()
                .map(|f| json!({ "name": f.name(), "label": f.label() }))
                .collect::<Vec<_>>(),
            "params": self.params.iter().map(param).collect::<Vec<_>>(),
            "values": self.values,
            "learned": self.learned,
            "activity": self.activity.iter().copied().map(entry).collect::<Vec<_>>(),
            "faults": self.faults,
            "latency": self.latency,
            "saved": self.saved,
            "kept": KEPT,
            "load": self.first_load.map(first_load),
            "started": self.started,
            "audio": self.audio.json(),
        });
        format!("window.update({state})")
    }
}

impl AudioShown<'_> {
    fn json(&self) -> Json {
        json!({
            "device": self.device,
            "deviceRate": self.device_rate,
            "channels": self.channels,
            "rates": self.rates
                .iter()
                .map(|&rate| json!({ "rate": rate, "label": khz(rate) }))
                .collect::<Vec<_>>(),
            "rate": self.settings.rate,
            "maxes": MAX_BLOCKS,
            "max": self.settings.max,
            "plans": Blocks::ALL
                .iter()
                .map(|&b| {
                    json!({
                        "name": b.name(),
                        "label": b.label(),
                        "text": b.text(),
                        "preview": preview(b, PREVIEW_MAX, PREVIEW_BUFFER, PREVIEW_BLOCKS),
                    })
                })
                .collect::<Vec<_>>(),
            "previewMax": PREVIEW_MAX,
            "blocks": self.settings.blocks.name(),
            "signals": Signal::ALL
                .iter()
                .map(|s| json!({ "name": s.name(), "label": s.label() }))
                .collect::<Vec<_>>(),
            "signal": self.signal.name(),
            "processing": self.processing,
            "played": self.played,
            "recent": self.recent,
            "note": self.note,
        })
    }
}

fn param(param: &Param) -> Json {
    json!({
        "title": param.title,
        "handle": param.handle,
        "kind": param.kind,
        "steps": param.steps,
    })
}

fn entry(entry: &Entry) -> Json {
    let at = u64::try_from(entry.at.as_micros()).unwrap_or(u64::MAX);
    match &entry.what {
        What::Call(call) => json!({
            "number": entry.number,
            "at": at,
            "kind": "call",
            "name": call.name,
            "fields": call.fields,
        }),
        What::Note(text) => json!({
            "number": entry.number,
            "at": at,
            "kind": "note",
            "text": text,
        }),
    }
}

fn first_load(load: &FirstLoad) -> Json {
    match load {
        FirstLoad::Nothing => json!({ "kind": "nothing" }),
        FirstLoad::Loaded { upgraded: None } => json!({ "kind": "loaded" }),
        FirstLoad::Loaded {
            upgraded: Some((from, to)),
        } => json!({ "kind": "upgraded", "from": from, "to": to }),
        FirstLoad::Refused { reason, block } => json!({
            "kind": "refused",
            "reason": reason,
            "block": super::block_text(block),
        }),
        FirstLoad::Unreadable { reason } => json!({ "kind": "unreadable", "reason": reason }),
    }
}

/// What the bar shows: the format, the audio and the faults, in a line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BarShown {
    /// The format the plugin runs through.
    pub(crate) format: Format,
    /// The audio it runs with.
    pub(crate) settings: Settings,
    /// Whether the plugin processes.
    pub(crate) processing: bool,
    /// The instance's faults.
    pub(crate) faults: u64,
    /// What the window shows of the plugin's editor below the bar.
    pub(crate) editor: EditorShown,
}

impl BarShown {
    /// Returns the line beside the format: the sample rate, the most frames
    /// a block holds, or that processing stopped, and the faults.
    pub(crate) fn status(&self) -> String {
        let faults = match self.faults {
            0 => "no faults".to_owned(),
            1 => "1 fault".to_owned(),
            many => format!("{many} faults"),
        };
        if !self.processing {
            return format!("stopped, {faults}");
        }
        let rate = khz(self.settings.rate);
        format!("{rate}, up to {} frames, {faults}", self.settings.max)
    }

    /// Returns the script that hands it to the bar's page.
    pub(crate) fn script(&self) -> String {
        let note = match (self.editor, self.format) {
            (EditorShown::Open, _) => None,
            (EditorShown::Failed, _) => Some(
                "The plugin's editor did not open: its parameters are in the settings window."
                    .to_owned(),
            ),
            // VST3 has views: the plugin offers none.
            (EditorShown::None, Format::Vst3) => Some(
                "The plugin has no editor: its parameters are in the settings window.".to_owned(),
            ),
            (EditorShown::None, Format::Cabi) => Some(format!(
                "{} has no editor: the plugin's parameters are in the settings window.",
                self.format.label()
            )),
        };
        let state = json!({
            "format": self.format.label(),
            "status": self.status(),
            "faults": self.faults,
            "note": note,
        });
        format!("window.update({state})")
    }
}
