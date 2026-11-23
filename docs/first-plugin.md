# Your first plugin

This guide walks through [`examples/gain`](../examples/gain/src/lib.rs), a
stereo gain that uses the parts of Hapto most plugins need: parameters, saved
state, undo, a value that is not a parameter, MIDI learn, and the VST3 export.
It leaves one thing out: nothing smooths its gain. Its editor, a page in the
host's window, comes last. Read it beside the source,
which fits one sitting. The guide follows the order in which you would write
the plugin, and links the item that owns each rule rather than repeating it.
Every block of Rust and TOML below is quoted from the example or its tests,
and its `guide` test fails when the two drift apart.

The plugin has four things to set:

- a gain in dB, which MIDI learn can drive;
- a bypass;
- a mode with three names: Clean, Warm and Hot;
- a label, saved with the project and undoable.

The example EQ (`examples/eq`) is the other example: parameters and saved
state only, the reference for performance and for parity with plain JUCE.

## The crate

A plugin is one library crate built two ways: a `cdylib`, the module a host
loads, and an `rlib`, which its tests drive in-process. The parts of the
example's [manifest](../examples/gain/Cargo.toml) that a plugin needs:

```toml
[lib]
# The cdylib is the VST3 module; the rlib lets tests drive it in-process.
crate-type = ["cdylib", "rlib"]

[features]
default = ["vst3"]
# The VST3 module's entry points, through Hapto's exporter.
vst3 = ["hapto-host/vst3"]
# The gain as an app of its own, which `cargo hapto dev` runs; the module
# never links it.
standalone = ["dep:hapto-standalone"]

[dependencies]
hapto-host = { workspace = true, features = ["manifest"] }
hapto-learn.workspace = true
hapto-params = { workspace = true, features = ["derive"] }
hapto-standalone = { workspace = true, optional = true }
hapto-undo.workspace = true
hapto-webview.workspace = true

[dev-dependencies]
hapto-host = { workspace = true, features = ["vst3", "test-host"] }
hapto-ump.workspace = true
```

`hapto-host` builds no exporter by default, so the crate's own `vst3` feature
turns the VST3 one on, and the tests add the test host. Its `manifest` feature
gives `manifest_info!`, which [section 3](#3-the-plugin-and-its-parts) uses.
`hapto-params`'s `derive` feature gives `#[derive(Params)]` and
`#[derive(Choices)]`. `hapto-webview` shows the plugin's page in the host's
window (section 9). `hapto-standalone`, behind the crate's own `standalone`
feature, runs the plugin as an app of its own while you work on it ([section
11](#11-run-it-while-you-edit-it)). Each `workspace = true` takes its value
from the repository's root manifest; [your own crate](#your-own-crate) says
what replaces it outside this repository.

## 1. Declare the parameters

A parameter is declared once, where the plugin holds it. The gain's sit in a
struct of their own, which derives [`Params`](../crates/params/src/lib.rs):
the mark on each field declares that parameter, and the derive writes the
function that declares them all and keeps what each is handed:

```rust
#[derive(Debug, Clone, Params)]
pub struct GainParams {
    #[param(
        "gain", "Gain", -60.0..=12.0, 0.0,
        step = 0.1, unit = "dB",
    )]
    gain: Float,
    #[param("mode", "Mode", Mode::Clean)]
    mode: Choice<Mode>,
    #[param("bypass", "Bypass", false, bypass)]
    bypass: Bool,
}
```

Each mark is a key, a display name, a range for a float, and a default in
real units, then its options. Everything else follows from it: the host's id
(a hash of the key), its range and text, the save, and the page's typed hook.

- The field's type is the parameter's kind, and says which declaration the
  mark writes: a [`FloatDecl`](../crates/params/src/declare.rs) for a `Float`,
  a `BoolDecl` for a `Bool` and a `ChoiceDecl` for a `Choice<Mode>`. The
  mark's leading items are that declaration's `new`, `step = 0.1` is its
  `with_step(0.1)`, and the lone word `bypass` its `as_bypass()`. So a
  float's arguments on a `Bool` field do not compile.
- A float's range reads as JUCE's `NormalisableRange (start, end, interval)`:
  ends included, and `step`, where 0, the default, means no steps.
  `skew_centre` puts a value at the middle of the control's travel, as
  `setSkewForCentre` does. So the gain runs from -60 dB to 12 dB in steps of
  0.1 dB, linearly. A default outside the range is refused, not moved into it.
- Each declaration is checked while compiling: a key or a range no parameter
  could have, a default outside its range, and a key another field has fail
  `cargo check`, on the mark. rustfmt leaves a mark as it is typed, so a long
  one is wrapped by hand, as the gain's is: the arguments, then the options.
- The store keeps real units (dB, not 0 to 1). A host speaks in 0 to 1
  positions, and Hapto converts both ways at its boundary.
- A choice's names are its labels, and the host sees them as the parameter's
  text. The mode's come from the enum that names its values, so the two cannot
  drift apart.
- `bypass` tells the exporter this parameter is the host's bypass. The
  processor still implements it: the host only learns which parameter it is.
- Hosts list the parameters in the order of the fields.
- The key is the parameter's identity for good. The saved block stores the
  value under it and the host's id is its hash, so a key never changes once
  the plugin ships, though the display name may. Adding a parameter later is
  safe: a block saved before it loads with the new parameter at its default
  ([`SavedState::load`](../crates/host/src/state.rs)).

The derive writes what you would write by hand: a `declare` that adds each
field's declaration, `p.add(FloatDecl::new(...).with_step(0.1))`, and returns
the struct. Write it yourself when the parameters are not one per field. The
example EQ's [`EqParams`](../examples/eq/src/params.rs) does: its six bands
come from a table, a band's shape is not a parameter, and a high-pass has no
gain.

`Mode` names the choice's values, and derives
[`Choices`](../crates/params/src/lib.rs):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Choices)]
enum Mode {
```

The derive writes the labels, each a variant's name unless a
`#[choice("...")]` mark on the variant gives another, and the mappings between
a mode and its index, in the order of the variants. `Hot` takes every index
past the last, since the audio thread may read any.

That order is the save format: a mode is saved as its index. A variant added
at the end keeps saved projects right, but moves every position the host
sees, so automation recorded against the shorter list plays other values. A
variant moved, or inserted before the last, needs a higher `SCHEMA_VERSION`
and a `migrate` that maps the old index to the new (section 3), or an older
project loads the wrong mode.

Each field is a [typed parameter](../crates/params/src/param.rs) that carries
its store: `get` returns the gain in dB as an `f32`, the mode as a `Mode` and
the bypass as a `bool`, on any thread, with no lookup or threshold. A clone is
the same parameter, sharing the store. Hapto calls `declare` when it builds an
instance, and a declaration it refuses, such as a key two parameters share,
fails that build with the reason, in any test.

## 2. Write the processor

[`Processor`](../crates/host/src/plugin.rs) is the audio thread's half, your
`processBlock`. The host calls `process` with a
[`Block`](../crates/host/src/audio.rs): its
[`Audio`](../crates/audio/src/audio.rs), edited in place, and the block's MIDI
events. The processor names its channels as a type, its
[layout](../crates/audio/src/layout.rs). The gain is `Stereo`, a `Bus` of two
channels in and two out, so its audio always holds two, and its loop walks
them with `block.audio.channels_mut()`, with no index or frame count. In a
`Bus` the channels are an array, so `let [left, right] =
block.audio.channels_mut()` would name each. A plugin that runs at several
widths takes `AnyLayout` and lists them in its `INFO`:

```rust
impl Processor for GainProcessor {
    type Layout = Stereo;

    fn process(&mut self, mut block: Block<'_, Stereo>) -> Status {
```

The block is `mut` because the processor writes its audio.

`GainProcessor` keeps the parameters' values in plain fields and reads them
again only when one moved. It holds a clone of the parameters and a
[`Changes`](../crates/params/src/changes.rs), which keeps a stamp of the
parameters it was last given and their change counts, and answers whether any
of them took a write since it last asked:

```rust
fn retune(&mut self) {
    let GainParams { gain, mode, bypass } = &self.params;
    if !self.changes.moved([gain, mode, bypass]) {
        return;
    }
    self.gain = 10.0_f32.powf(gain.get() / 20.0);
    self.mode = mode.get();
    self.bypassed = bypass.get();
}
```

The parameters carry their stores, so the processor needs no handle to the set
to read them. Reading takes atomics only, and the audio thread never
allocates, locks or waits ([the invariants](../ARCHITECTURE.md#invariants)).
The loop itself is plain DSP.

Open: Hapto has no smoothing yet. The gain is read once per block, at its
start, so a change steps there, and a large one can click.

## 3. The plugin and its parts

[`Plugin`](../crates/host/src/plugin.rs) is the main thread's half. Its
constants name the plugin (`INFO`), and its `Params` is the struct above.
Hapto declares it, builds the parameter set, the session and the saved state
over it, and hands the parameters and those
[`Parts`](../crates/host/src/parts.rs) to `new`. `new` keeps what the plugin
uses; `activate` builds a fresh processor each time the host activates it, so
nothing the audio thread touches is allocated in a block:

```rust
fn activate(&mut self, _setup: &ProcessSetup) -> GainProcessor {
    GainProcessor::new(self.params.clone())
}
```

The host saves and loads through the saved state in the parts, on threads of
its own, without touching the plugin. The saved block is JSON: Hapto's version
and the plugin's own, the parameters' values, and one keyed section per
session participant. A later version of the plugin that renames or rescales a
parameter raises its `SCHEMA_VERSION` and upgrades an older block in its
`migrate`, as it loads.

The parts hold the main thread's handles, so a plugin that keeps them is not
`Send`, and runs on the thread that created it. The example EQ keeps only its
parameters and drops its parts, so it stays `Send`.

`INFO` is what a host reads before it creates an instance: the name it lists,
the ids it knows the plugin by, the category its browser files it under. Who
the plugin is sits in the manifest, with the package's version and
description, which are the plugin's:

```toml
[package.metadata.hapto]
name = "Hapto Example Gain"
vendor = "Hapto"
id = "dev.hapto.example-gain"
vst3-class-id = "5b0ad271-3e96-4c18-a37f-e0248db56a19"
au-codes = { subtype = "SmGn", manufacturer = "Hapt" }
category = "effect"
```

[`manifest_info!`](../crates/host/src/lib.rs) reads it into `INFO` and checks
it while compiling, since Cargo reads no `[package.metadata]` table itself.
What the code relies on, the MIDI it takes, is written beside the code. The
processor's `Stereo` already says the width it runs at:

```rust
const INFO: PluginInfo = hapto_host::manifest_info! {
    midi_input: true,
    midi_output: false,
};
```

A host knows the plugin by its ids for good:
[`vst3_class_id`](../crates/host/src/info.rs) says how to make one, and why it
never changes.

## 4. Undo

Undo is one call in `new`, which turns it on with every parameter enrolled:

```rust
let history = parts.undo(Bound::default());
```

It returns no `Result`. Its one failure, a history the plugin made of its
own, is a mistake in the plugin's source, so it panics with what to change,
and an exporter then refuses to build the instance. The style guide's
[rule](style.md#haptos-own-rules) says which calls panic and which return a
`Result`.

The `History` it returns records changes to what is enrolled in it. Only
writes through the parts' [`UiParams`](../crates/params/src/ui.rs) handle,
the one an editor writes parameters through, record, as a user's: a host's
automation, a load and a learned controller never do, so undo never fights
the host. A drag is one [gesture](glossary.md#gesture) and so one undo step,
and so is each lone `write`, a click say, which is a gesture of one value. The
history records once per tick, but an undo first records what changed since
the last one, so it needs no tick before it. From the example's tests:

```rust
let gain = plugin.ui().set().id(GAIN).expect("declared");

// Each lone write is a gesture of its own, so each is a step. Undo
// records what changed since the last tick before it undoes.
assert!(plugin.ui().write(gain, -6.0));
assert!(plugin.ui().write(gain, -12.0));
assert_eq!(plugin.history().undo(), Ok(true));
assert_eq!(plugin.ui().value(gain), -6.0);
```

## 5. A value that is not a parameter

The label is state the host knows nothing about. A
[`Value<String>`](../crates/params/src/value.rs) holds it, and two statements
in `new` give it a life:

```rust
let label = parts
    .saved_value(LABEL_KEY, DEFAULT_LABEL.to_owned())
    .expect("a free key");
history.enroll(&label, "Rename").expect("a new value");
```

Enrolling it makes its changes undo steps: the sets between two ticks are one
step, the latest winning, where two lone writes of a parameter are two. A
[`List`](../crates/params/src/list.rs)'s verbs are one step each instead. The
same test goes on with the label:

```rust
// The label's sets between two ticks are one step, the latest winning.
plugin.set_label("Vocals");
plugin.set_label("Lead vocals");
assert_eq!(plugin.history().undo(), Ok(true));
assert_eq!(plugin.label(), "Untitled");
```

[`Parts::saved_value`](../crates/host/src/parts.rs) makes it and saves it with
the project under a stable key, `"label"`, as JSON through serde, so any value
serde can write and read is saved the same way. The default is written once:
a new instance starts with it, and a project saved before the label existed
loads it. Undo is a separate call, since a value such as a view setting is
saved and not undone. A change, an undo's included, reaches the saved block
at the next tick; that method says why and what it costs. A load writes the
parameters at once, before it returns, but the label at the next tick.

The host lists no label, so no edit tells it of a rename. Hapto tells it at
that tick that the project changed, so the host asks to save a project in
which only the label moved. A load is not such a change.

## 6. MIDI learn

Learn is one more call in `new`:

```rust
parts.midi_learn(Config::default());
```

It registers learn's own participant, so its mappings save under their own
key. Like `undo`, it panics on a mistake in the plugin's source: learn turned
on twice, or a plugin that declares no MIDI input. Hapto runs the rest: each
activation takes learn's audio side, a
[`LearnAudio`](../crates/learn/src/audio.rs), and each block runs it over the
block's MIDI before the processor. Learned writes then land before the
parameters are read, in the block the controller arrived in, and the
processor holds nothing of learn.

To learn, the plugin calls `Learn::arm`, through `parts.learn()`, on the
gain's id, the parameter's `id()`, with `Takes::CONTROLLER`. The next
controller a block brings is captured on the audio thread, and the next tick
commits it.

The plugin declares `midi_input: true` in its `INFO`; learn refuses a plugin
that does not. Open: no exporter delivers MIDI events yet, so learn is
exercised by a test that runs learn's audio side itself, as the tests below
do.

## 7. The idle tick

About 60 times a second on the main thread, Hapto ticks the parts the plugin
keeps: [`Driver::tick`](../crates/host/src/parts.rs) runs learn, the session,
the history and the saved values in the one order that works, and owns the
reasons for it. The plugin writes nothing for it, and the gain writes no
`idle`. `Plugin::idle` is for a plugin's own main-thread work, and runs right
after Hapto's tick, so it sees a restore applied and a learned mapping
committed.

## 8. Test it through a host

`hapto_host::vst3::TestHost` drives the plugin through the VST3 exporter's own
interfaces, as a host would.
[`examples/gain/tests/gain.rs`](../examples/gain/tests/gain.rs) uses it for
what a host can reach: it sets up, activates and starts the host, each call
answering `Ok(())` or the plugin's refusal, then calls `process_in_place` with
changes given by host id, in the host's 0 to 1 (`normalize` converts dB first),
reads the modes' names through `parameter` and `text`, and carries the saved
block through `save` and `load`.

Learn and undo are driven on the plugin itself, since a host cannot reach
them, nor yet send MIDI. The test builds the plugin as an exporter does,
keeping a [`Driver`](../crates/host/src/parts.rs) to do what the exporter's
engine would: tick the parts, and run learn's audio side before each block.
It keeps the saved state a host would save and load through too:

```rust
fn built() -> (Gain, Driver, Arc<SavedState>) {
    let (params, parts) = Parts::for_plugin::<Gain>().expect("valid declarations");
    let driver = Driver::of(&parts);
    let saved = Arc::clone(parts.saved());
    (Gain::new(params, parts), driver, saved)
}
```

Then it activates the plugin and takes learn's audio side from the driver,
as an activation does, arms the gain, runs a controller through learn's audio
side and then the processor, ticks the driver, and saves and reloads to check
the mapping survives. Run the example's
tests, this guide's check among them, with:

```sh
cargo test -p hapto-example-gain
```

[docs/testing.md](testing.md) says what each CI lane proves.

## 9. A page in the host's window

The gain's editor is a page: a slider on the gain, a field for the label and
the plugin's version, a small Vite and React app in
[`examples/gain/page`](../examples/gain/page/src/main.tsx) on the `@hapto/page`
hooks, typed by the gain's parameter map, which a test of the gain keeps in
step with its declarations. `npm run build` there writes three files under
fixed names, which
[`src/page.rs`](../examples/gain/src/page.rs) builds into the plugin when they
are there. When they are not, `build.rs` warns and the plugin offers no
editor but a dev server's (below), until `npm run build` writes them and the
next Cargo build takes them in: Cargo never runs Node. The plugin hands the
editor out:

```rust
fn editor(&mut self) -> Option<Box<dyn Editor>> {
    page::editor(self)
}
```

[`Page`](../crates/webview/src/lib.rs), from `hapto-webview`, makes it from the
built files, a size, the label and one command:

```rust
pub(crate) fn editor(gain: &Gain) -> Option<Box<dyn Editor>> {
    Page::new(&Gain::INFO, gain.ui().clone(), PAGE.unwrap_or_default())
        .with_size(Size {
            width: 420,
            height: 220,
        })
        .value("label", &gain.label)
        .command("version", |()| Ok(Gain::INFO.version))
        .into_editor()
}
```

`value` binds the label for the page: the same value the plugin saves and
undo records, which the page reads with `useValue("label")`. The page shows
it as the plugin holds it, whatever changed it, a load or an undo included,
and a rename from the page is the user's write, which undo records as it does
the plugin's own sets and the plugin saves. A value crosses as the JSON
serde writes for its type, and the page's `Values` map, written by hand,
gives each key that type. The field keeps the text being typed only until
Enter or the field loses the focus, and stores it only when it differs from
the label, so a load while the field is idle shows at once and a click into
the field and out stores nothing. Escape drops the text typed and shows the
label again. A command is for what is not state, such
as the version the page asks for once and shows beside its title, or a write
the plugin must check or refuse.

The page writes the gain through a clone of the parts' handle, as a user
would: one host gesture and one undo step per drag. The VST3 exporter opens
the editor in the host's window on Windows, follows its size and scale, and
ticks it after each idle; elsewhere `into_editor` gives `None`, and the host
shows its own controls. A debug build loads the page from Vite's dev server,
with hot reload, when `HAPTO_PAGE_DEV_URL` holds the address `npm run dev`
prints, whether or not the page was built in; with no files and no dev
server, `into_editor` gives `None`.

The page takes the keyboard when it is clicked, and gives it back to the host
once nothing on it needs keys: after a click on the background or a drag of
the slider, the space bar starts the host's transport again, and while the
label field has the focus, every key is the field's. A field, a `select` and
editable content keep the keys with no code of the gain's. A control of a
page's own that takes keys, such as a knob on a canvas that turns with the
arrow keys, says so with an attribute, on itself or on any element around it:

```tsx
<canvas tabIndex={0} data-hapto-keys />
```

## 10. Build and bundle the module

`export_vst3!(Gain)` at the top of the crate's `lib.rs` turns the `cdylib` into
a VST3 module, under the crate's own `vst3` feature from the manifest above.
One command, in any shell, builds the page, then the module in release, lays
out its bundle and installs it:

```sh
cargo hapto build -p hapto-example-gain --install
```

`cargo hapto` is the plugin author's command, the `cargo-hapto` crate in
`crates/cli`; this repository runs it through an alias, and [your own
crate](#your-own-crate) installs it. Run from inside a plugin crate's folder,
or in a workspace with one plugin, it needs no `-p`. It runs `npm ci` in
`page/` when `node_modules` is missing or older than the lockfile, or `npm
install` when the page has no lockfile yet, then `npm run typecheck`, when the
page has that script, and `npm run build` every time. Vite's build strips the
page's types without checking them, so the check is what stops a page whose
keys disagree with its generated parameter map; neither that nor a stale page
reaches the bundle.
`--no-page` skips the page for a Rust-only rebuild.

`--debug` bundles the debug build instead, for working on the page in the
host: run `cargo hapto serve`, and start the host with `HAPTO_PAGE_DEV_URL` set
to the address it prints, and the page reloads as you edit it. From
PowerShell, `$env:HAPTO_PAGE_DEV_URL = "http://127.0.0.1:5173"`, then the
host's executable from that shell. The DSP runs unoptimised in that build, so
listen to a release bundle. The bundle goes to
`target/bundle/Hapto Example Gain.vst3`, named from `[package.metadata.hapto]`.
`--install` copies it to the per-user VST3 folder,
`%LOCALAPPDATA%\Programs\Common\VST3`, which needs no administrator;
`--install-system` copies it to `C:\Program Files\Common Files\VST3` instead,
from an administrator's shell. Either warns when the other folder holds a
bundle of the same name, since the host then picks one. Rescan in the host,
and it opens the page as the plugin's editor.

What the command lays out: a VST3 bundle is a folder, and on Windows the
library goes in as the module itself, under the architecture's folder, as CI
lays out the EQ's bundle:

```sh
cd examples/gain/page && npm ci && npm run typecheck && npm run build && cd ../../..
cargo build -p hapto-example-gain --release
bundle="target/bundle/Hapto Example Gain.vst3"
mkdir -p "$bundle/Contents/x86_64-win"
cp target/release/hapto_example_gain.dll \
  "$bundle/Contents/x86_64-win/Hapto Example Gain.vst3"
```

The editor needs the WebView2 runtime, which Windows 11 ships; where it is
missing, the editor does not open, the fault report says why, and the audio
goes on. The page keeps its browser data in
`%LOCALAPPDATA%\dev.hapto.example-gain\WebView2`.
[pluginval](https://github.com/Tracktion/pluginval) checks a bundle without a
host. CI runs version 1.0.4 on the EQ's bundle with these options; for the
gain's:

```sh
pluginval --strictness-level 5 --validate-in-process --skip-gui-tests \
  "target/bundle/Hapto Example Gain.vst3"
```

`cargo hapto validate -p hapto-example-gain` builds the release bundle and
runs that, with the pluginval on the `PATH`, or the one `PLUGINVAL` or
`--pluginval` names.

Once a plugin's editor has opened, a host keeps the module's file in use until
the host exits, so quit the host before installing the bundle again: the
command's copy fails until then, and says so.

Open: CI lays out and validates the EQ's Windows bundle only, not the gain's.
The macOS bundle is not built yet, and the command refuses there.

## 11. Run it while you edit it

A host keeps a plugin's module loaded until it exits, so a change to Rust
means quitting it and installing again. The gain also builds as an app of its
own, with no host: a binary behind its `standalone` feature, at the end of its
manifest,

```toml
[[bin]]
name = "hapto-example-gain-standalone"
path = "src/bin/standalone.rs"
required-features = ["standalone"]
```

whose `main` is one call:

```rust
fn main() -> std::process::ExitCode {
    hapto_standalone::main::<hapto_example_gain::Gain>()
}
```

[`hapto_standalone::main`](../crates/standalone/src/lib.rs) runs the plugin
through Hapto's standalone exporter, the same engine the VST3 exporter
drives: it opens the plugin's editor in a window, plays a test signal through
the plugin to the system's audio output, and ticks it as a host's timer
would. One command runs it:

```sh
cargo hapto dev -p hapto-example-gain
```

It starts the page's dev server, writes the page's parameter map from the
declarations through the `page_params` test, builds the binary in debug and
runs it with the page from the server, so an edit to the page shows at once.
An edit to any member's Rust rebuilds it while the app plays on, then asks
the app to quit and starts the new build. The app keeps the plugin's saved
block in `target/hapto/dev/hapto-example-gain/` as it quits and each second
it changed, and loads it as it starts, so the gain, the mode and the label
come back after each rebuild. Close the window to end it. The plugin's
inputs hear silence unless you ask for a test signal: `--signal sine`, a 440
Hz sine at -24 dBFS, or `--signal noise`, white noise at the same level.
`--release` runs the optimised build, to listen to the DSP.

`--host` alone runs the same binary through the dev host instead: the gain
goes through a plugin format, as a host meets it, in process. In VST3 it is
the exporter's own COM interfaces, ticked by the exporter's timer, with its
editor as the exporter's view; in the C interface, the table the JUCE adapter
drives, with no editor. The window is the gain's, as in a host, with a bar
across the top that names the format and the audio. Its Settings button
opens the settings window, a tab for each thing the host sees. Its header
switches the format: the app quits keeping the saved block and the format,
and the command starts it again in the other, so a block one format saved
loads in the other. The Audio tab changes what a host changes: the sample
rate, of those the device takes, and the most frames a block holds, each by
reactivating the plugin as a host does, and how blocks are cut, as the
device asks, at the maximum, at random, or at awkward lengths such as 1
frame, none, and one past the maximum, which a graph of the last blocks
shows. The sound is the same whatever the cut, so a click or a change in it
is the plugin's. It also stops and starts processing, and picks the test
signal. Parameters lists them as the host does, with a control for each.
Activity logs what the plugin told the host, the gestures, the restarts and
a change to state the host does not list, and what the dev host did to the
plugin. State shows the saved block and whether
the plugin took it; a block it refused is kept for the next run rather than
saved over. The binary needs nothing more for it: the command builds it with
`hapto-standalone`'s `dev-host` feature.

In a real host, the debug bundle loads its page from the dev server too.
`cargo hapto build --debug --install` installs the bundle, `cargo hapto
serve` runs the server until Ctrl+C, and the host, started with
`HAPTO_PAGE_DEV_URL` set to the address `serve` prints, reloads the page as
it changes. A change to Rust needs the bundle built and the host started
again; the server goes on. The dev host stands in for none of these: a real
host's quirks are what it cannot show.

The app runs on Windows only, as the page does. No MIDI reaches the plugin,
nor any automation: the app is the host, and an edit the page makes lands in
the parameters with no one else to tell.

## Your own crate

Install the command from a checkout of Hapto, then let it write the crate:

```sh
cargo install --path hapto/crates/cli
cargo hapto init my-plugin --vendor "My Company"
cd my-plugin
cargo hapto dev
```

`init` writes a smaller plugin than the gain, in a folder of its own: a gain
in dB and a bypass, its page with one slider, the standalone binary of
section 11, the page's parameter map with the `page_params` test that keeps
it, and a test through the VST3 exporter (section 8). The crate is a
workspace of its own; delete its `[workspace]` table to make it a member of
another. Its identity comes from the folder's name and the flags: the
package, the type (`MyPlugin`), the name hosts show (`--name`, `My Plugin`
by default), the vendor, a reverse-DNS id (`--id`, `com.mycompany.my-plugin`
by default), a fresh UUID for `vst3-class-id`, a fresh AU subtype and a
manufacturer code from the vendor. A host knows a plugin by its ids for good:
a crate copied from another plugin, the gain among them, keeps that plugin's,
and every host takes one for the other.

Hapto's crates are not published yet, so the crate takes them, and its page
takes `@hapto/page`, from a checkout by path: the one `--hapto` names, or the
one the command was built from. Every Hapto crate comes from the same place,
since before 1.0 a minor version may break the API, and the command warns
when the crate's lockfile pins another `hapto-host` than its own version.

## Where to read on

- [ARCHITECTURE.md](../ARCHITECTURE.md) follows a host's call to your block.
- Each crate's overview has its invariants and one worked example:
  [params](../crates/params/src/lib.rs), [undo](../crates/undo/src/lib.rs),
  [session](../crates/session/src/lib.rs), [learn](../crates/learn/src/lib.rs)
  and [host](../crates/host/src/lib.rs).
- The [glossary](glossary.md) defines the words Hapto uses in a sense of
  its own.
- The example EQ shows a larger processor, with the allocation gates and the
  parity tests that prove it.
- [docs/style.md](style.md) is how code and docs are written here.
