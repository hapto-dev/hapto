# Architecture

Hapto is a Rust framework for audio plugins. A plugin declares each parameter
once, and gets automation, undo, MIDI learn, saved state and a typed React page
from that one declaration. Hosts see the plugin through an
[exporter](docs/glossary.md#exporter): VST3 natively, and AU, AUv3, AAX and
hosting through a generic JUCE adapter driven by a C interface. A UI is one
React page in a WebView, talking to Rust through a transport the page does not
choose.

Three [tiers](docs/glossary.md#tier), dependencies pointing down only. The core
is `params`, `undo`, `session`, `learn`, `ump` and `audio`. The projections are
`bridge` (the page) and `host` (plugin formats). Shells (a plugin crate, the
JUCE adapter) sit on top and may depend on both. Each rule below links the entry
of [the decision record](docs/decisions.md) that says why it exists, where the
record has one, and [docs/decision-index.md](docs/decision-index.md) leads from
each decision to its code. A word Hapto coins links to [the
glossary](docs/glossary.md) where it first appears. To write a plugin before
reading how Hapto works inside, start with
[docs/first-plugin.md](docs/first-plugin.md).

## Codemap

- `crates/params`: parameter declarations, [typed
  parameters](docs/glossary.md#typed-parameter), the value
  [store](docs/glossary.md#store), write [origins](docs/glossary.md#origin),
  [gestures](docs/glossary.md#gesture), change counters, and the declarer that
  makes [parameters at run time](docs/glossary.md#parameter-made-at-run-time),
  in no set.
- `crates/macros`: `#[derive(Params)]` and `#[derive(Choices)]`, which
  `params` re-exports under its `derive` feature, and `manifest_info!`, which
  `host` re-exports under its `manifest` feature and which reads a plugin's
  identity from its manifest: a proc-macro crate below the core, on `syn`, and
  for the manifest on `toml`'s parser.
- `crates/undo`: undo history over parameters, plain values and lists.
- `crates/session`: saved state: who contributes to it,
  [restore](docs/glossary.md#restore), late
  [participants](docs/glossary.md#participant).
- `crates/learn`: MIDI learn and the audio thread's dispatch table. Its
  capture and dispatch words are packed by `field.rs`, the controllers a plugin
  reserves are in `reservation.rs`, and the section it saves is
  `mappings/session.rs`.
- `crates/ump`: Universal MIDI Packets, the event list MIDI travels in, and
  MIDI 2.0's channel voice messages, which everything above the exporters
  reads (`voice.rs`). An exporter translates MIDI 1.0's (`midi1.rs`) up on the
  way in (`upgrade.rs`) and down on the way out (`downgrade.rs`), and
  `scale.rs` moves values between the two protocols' widths. A device's MIDI
  1.0 byte stream becomes packets, and packets bytes, in `bytes.rs`.
- `crates/midi`: MIDI in musicians' terms: the values a message carries,
  each its own type (`values.rs`), the messages (`message.rs`), the decoder
  that reads packets into them (`decoder.rs`), a block's MIDI in and out,
  `Midi` and `MidiOut` (`midi.rs`), the encoder that writes a message back as
  packets (`encode.rs`), the controllers, typed (`controller.rs`), and a
  block's audio cut at each message, its [spans](docs/glossary.md#span)
  (`spans.rs`), and an instrument's voices (`voices/`): `Poly` and the
  `Voice` trait (`poly.rs`, `mod.rs`), what a note plays at (`expression.rs`,
  `tuning.rs`) from what its channel and its key keep (`state.rs`), MPE's
  zones (`mpe.rs`), and the audio a voice renders into (`audio.rs`).
- `crates/audio`: a block's audio as a processor sees it: the layouts, the
  channels a main bus has as a type (`layout.rs`); `Audio`, the channels a
  block lends, walked by frame or by channel (`audio.rs`, `frame.rs`); and
  `AudioBuf`, audio of its own for tests (`buf.rs`); and the `Status` a block
  answers (`status.rs`). `host` re-exports them.
- `crates/host`: the `Plugin` trait, a block, which makes its spans
  (`audio.rs`, `spans.rs`), the
  [parts](docs/glossary.md#parts) Hapto
  builds for each instance (`parts.rs`, with the parameters a plugin makes at
  run time in `parts/run_time.rs`), the saved block (`SavedState`), which
  keeps those parameters by [path](docs/glossary.md#path)
  (`state/run_time.rs`), and
  the exporters behind features: `vst3`, and `cabi` (the C interface and
  generated `include/hapto.h`). The exporters are thin shells over one core,
  `engine`, which holds the plugin's life and builds with no feature. CLAP is
  planned, not built. A plugin's editor is an `Editor` (`editor.rs`), which
  knows no toolkit; `engine/editor.rs` drives one for each view a host asks
  for, and the VST3 exporter's view is `vst3/view.rs`, with each system's
  window type in a file of its own in `vst3/view/`. The `standalone`
  exporter (`standalone.rs`) drives the same engine for an app with no host,
  and `Play` (`play.rs`, under `test-host`) drives it for a plugin's tests.
- `crates/bridge`: the page protocol, its native side (`Dispatcher`, which
  answers a page and tells it of every change to the parameters and to the
  values a plugin binds for it in `Values`), and the TypeScript generated from
  it.
- `crates/webview`: the shell that shows a plugin's page in the host's
  window, on Windows: `Page` makes the `Editor`, a WebView2 child window
  through wry, which serves the page from bytes built into the plugin and
  joins it to a `Dispatcher` (`editor.rs`). The page's native side is in
  `native.rs`. `platform.rs` defines the `WebEngine` trait, and each system's
  engine implements it in `platform/`.
- `crates/midi-devices`: `hapto-midi-devices`, the computer's MIDI devices as
  UMP, for the app: the system's list and its changes (`devices.rs`), what names
  a device (`id.rs`), the inputs open on the main thread and read into each
  block on the audio thread (`inputs.rs`), the outputs each block's MIDI goes
  to, sent by a thread of each output's own (`outputs.rs`), the ring between
  the threads, either way (`ring.rs`), and the WinMM backend (`winmm/`).
- `crates/standalone`: `hapto-standalone`, the plugin as an app of its own, on
  Windows: the standalone exporter's instance, its editor in a winit window, and
  a test signal through it to the system's audio output through cpal (`app.rs`,
  `device.rs`, `block.rs`), with the computer's MIDI inputs and outputs for a
  plugin that takes or sends MIDI (`midi.rs`), which its author's code picks
  (`routing.rs`), and its saved block kept between runs (`session.rs`). A plugin
  crate's binary calls its `main`, and its allocation gate reaches the audio
  path through a hidden `gate` module (`gate.rs`). Its dev host (`dev_host/`,
  the `dev-host` feature) runs the plugin through the VST3 or C interface test
  host instead, below a bar that names the format, with a settings window of
  what the host sees.
- `crates/testkit`: test tools: allocation gate, latches, expected assertions,
  drift check.
- `examples/gain`: the example gain, which wires parameters, undo, a saved
  value, MIDI learn, a page in the host's window (`page/`, `src/page.rs`) and
  the VST3 export in one plugin; [docs/first-plugin.md](docs/first-plugin.md)
  walks through it.
- `examples/eq`: the example EQ, the reference for performance and parity, as a
  VST3 module or a static library for the adapter.
- `examples/midi-probe`: a plugin that makes a host's MIDI heard and sends it
  back out, for checking MIDI through a real host by hand, in each format.
- `page`: React hooks for the page, over a pluggable transport: a memory
  one for tests, and `webviewTransport` for a page in a plugin's window. Its
  client holds writes made before the native side answers in `src/held.ts`,
  and limits a position to 0 to 1, which the bridge's decoding refuses
  outside.
- `adapters/juce`: the generic JUCE adapter: any Hapto plugin, through
  `hapto.h`, as a JUCE plugin.
- `crates/cli`: `cargo-hapto`, the plugin author's command, for a plugin crate
  in any workspace: `cargo hapto init` writes a new plugin crate from
  `template/`; `dev` runs the plugin's standalone binary and restarts it after
  each rebuild, with the page from its dev server (`dev.rs`, `watch.rs`);
  `serve` runs that server alone, for a debug bundle in a real host
  (`serve.rs`); `build` builds a plugin crate's page and module and lays out
  its VST3 bundle; `validate` runs pluginval on it. `main.rs` declares the
  command line, which clap reads. It links no Hapto crate: it runs Cargo, npm
  and Node.
- `xtask`: `cargo xtask gen` writes every generated file; each has a drift gate.
- `bench`: the comparison against plain JUCE.

## From a host's call to your block

Read these in order to change what happens to a block.

1. A plugin implements `Plugin` (`crates/host/src/plugin.rs`): its `INFO`, its
   `Params`, `new`, which takes the parameters and the `Parts` Hapto built from
   them (`parts.rs`), and `activate`, which builds the `Processor`.
   `examples/eq/src/lib.rs` is a whole one, `examples/eq/src/params.rs` its
   parameters; `examples/gain/src/lib.rs` adds undo, a saved value and learn.
2. An exporter (`vst3/component.rs`, `cabi/instance.rs`) turns the host's calls
   into calls on `Engine` (`crates/host/src/engine/mod.rs`), which holds the
   instance's life: building the plugin,
   [activation](docs/glossary.md#activation), the
   [tick](docs/glossary.md#tick), what the host is told, teardown. The table
   below maps each entry point to its `Engine` method.
3. A [block](docs/glossary.md#block) is `Engine::process` (`engine/process.rs`).
   On the audio thread it hands the exporter a `Points` over the engine's
   `Automation` to fill (step 4), applies it to the parameter set, and flushes
   denormals. Then `run_held` takes the processor out of its `AudioSlot`,
   and the exporter hands over the block's MIDI through a `MidiIn`, which
   upgrades MIDI 1.0 as it arrives with the activation's own `Upgrade`.
   When the plugin's parts have learn on, learn's audio side, which the
   activation took through the parts' `Driver`, runs once over the block's
   MIDI, so learned writes land before the processor reads. A learned write to
   a parameter made at run time goes through the store learn's table holds
   for it. The block's MIDI is then decoded once into the activation's
   `MidiList` (`crates/midi`), and `pieces` calls `Processor::process` with a
   `Block` (`crates/host/src/audio.rs`) in
   [pieces](docs/glossary.md#piece) of at most the activation's `max_frames`,
   each with its share of the messages as `block.midi`.
   `Channels::run` (`boundary/channels.rs`) builds each piece's channels in
   place, and `pieces` lends them to the processor as `Audio` in its
   layout. Channel `c` is the host's output buffer `c` when that overlaps no
   earlier channel's, and a scratch buffer otherwise. The host's input is
   copied into it unless it is already there. A scratch buffer also stands in
   for a channel the host gave none. The processor changes the audio in place.
4. The host's automation is `engine/automation.rs`. A VST3 host sends queues of
   points, each at a sample offset (`Point`); the exporter hands the engine one
   queue at a time through `Points::queue`. The engine has
   [room](docs/glossary.md#room) for 8 points per parameter on average, fixed
   when it is built. A parameter's first queue wins: a second queue for it is
   passed over unread. The engine reads the winning queue's last point first and
   applies it before the block, so the processor sees one value per parameter
   per block. Earlier points are carried while the room lasts and counted as
   dropped after that. A C-interface host sends no queues (`HostLink::QUEUES` is
   false), so the engine has no room and a write lands at once through
   `Engine::host_write`.
5. A parameter reaches the block through one atomic cell per parameter in
   `ParamSet` (`crates/params/src/set.rs`). A host write is a 0 to 1 position
   that `AudioParams::host_write` turns into a legal value
   (`ParamDecl::host_value`) and stores, with its origin, in one atomic word
   (`crates/params/src/set/value_cell.rs`). The processor reads
   `ParamSet::value` and compares `ParamSet::changes` at the top of its block,
   and recomputes only what moved (`retune` in the example EQ). A reader that
   needs a value, its count and its origin together asks `ParamSet::read`,
   which returns a `Reading`. A parameter made at run time has a cell of its
   own, in no set, and its typed parameter reads it the same way.
6. A write from the page or an editor goes the other way: `UiParams` stores into
   the same cell, and at the next tick the `HostEventDrain` (a
   [drain](docs/glossary.md#drain)) tells the host.
7. State the host does not list has no edit to tell: a saved value, a learned
   mapping, a parameter made at run time. When the plugin or its user changed
   any of it, the same tick tells the host only that its project changed
   (`Engine::tell_host`, `HostLink::state_changed`), after the parts captured
   it, so a save made then holds it. A load is never told ([the host is told
   of state it does not list][unlisted]).

`HostLink` (`engine/mod.rs`) is how a shell tells the engine what its host is
like. `QUEUES` says whether the host sends automation as queues with each block:
VST3 does, the C interface does not. `ACTIVATION_TELLS_LATENCY` says whether the
host reads the latency as an activation returns. The engine is generic over it,
so one engine serves both shells.

| Host's call | VST3 (`vst3/component.rs`; the controller half in `vst3/component/controller.rs`) | C interface (`cabi/mod.rs`) | Standalone (`standalone.rs`) | `Engine` method |
| --- | --- | --- | --- | --- |
| One block, with its automation | `process` | `process` | `StandaloneAudio::process`, with no automation | `process` |
| A write outside a block | `setParamNormalized` | `set` | none | `controller_write` (VST3), `host_write` (C) |
| Start or stop processing | `setProcessing` | none | none | `set_processing` |
| Reset the processor | a stop in `setProcessing` | `reset` | `reset` | `request_reset` |
| Activate or deactivate | `setActive` | `activate`, `deactivate` | `activate`, `deactivate` | `activate`, `deactivate` |
| A tick | the exporter's timer | `idle` | the app's `tick` | `tick`, `idle` |
| The editor's view | `createView`, then the view's `attached`, `onSize`, `setContentScaleFactor`, `removed` (`vst3/view.rs`) | none | `editor`, then `StandaloneEditor`'s `open`, `set_size`, `set_scale`, `close` | `editor`, then the view's `EditorSlot` |

To prove a change, run the tests that reach it. `engine/tests.rs` tests each of
the engine's rules through both exporters with one body.
`examples/eq/tests/matrix.rs` and `overflow.rs` drive the example EQ through
both exporters across block sizes, rates and layouts, and with automation that
overflows the room. The allocation gates (`alloc_gate.rs` in `crates/host/tests`
and `examples/eq/tests`) fail on an allocation in the block. The loom models
check the hand-offs (`scripts/run-loom.py`): the parameter cell, the host event
drain, learn's staged value and its capture staging, and in
`boundary/loom_models.rs` the main-thread state and the processor slot.
[docs/testing.md](docs/testing.md) says what each proves and how to run it, and
which audio-path modules deny the lints that refuse indexing, `unwrap` and
unchecked arithmetic.

The engine's items are `pub(crate)`, so `cargo doc` hides them. Read their docs
with `cargo doc -p hapto-host --features vst3,cabi,test-host
--document-private-items --open`.

## Invariants

- The core crates and `bridge` never depend on a window, WebView, UI toolkit or
  plugin format, with any features, on any platform.
  `scripts/check-core-deps.py` checks it in CI ([where the WebView pieces
  live][webview]).
- `undo` and `session` never depend on each other. Both take the same restore
  clock from the parameter set that owns it ([who makes the clock][clock]). A
  load moves the clock with every parameter it writes, under the lock a replay
  step checks it under, so no step for a change finished before a load replays
  over it ([a load and a replay][load]). A user's gesture still open when the
  load lands, a lone write included, keeps its value from before the gesture,
  and undoing it writes that value.
- `unsafe` is denied workspace-wide. Only `host`'s `vst3`, `cabi` and
  `standalone` modules and the `boundary`, `engine` and `faults` modules they
  share allow it, and `midi-devices`' system backend (`winmm`), with a
  `// SAFETY:` comment on every block.
- A panic never unwinds into a host. An exporter catches it at the entry point,
  counts it as a fault, and a processor that panicked plays silence until the
  next activation. The first fault of each kind in an instance is reported as a
  line of text, never on the audio thread
  ([`faults.rs`](crates/host/src/faults.rs) says when, and on which thread;
  [a panic in a host's call][panic], [making a fault visible][faults]).
- The audio path never allocates, locks or makes a system call, except while a
  panic unwinds (`faults.rs` says why) and where the standalone app's MIDI out
  sets each output's signal, an event whose setting never blocks, once a block
  has handed it MIDI (`crates/midi-devices/src/outputs.rs`).
  Allocation gates, and in a debug build the exporters' audio-thread mark, check
  what the tests reach;
  [testing.md](docs/testing.md#2-the-audio-path-does-not-allocate) says what
  each sees and what is left to review ([checking the audio-thread rule][mark]).
- A saved block names two versions, each its own: `haptoStateVersion` for the
  frame Hapto gives it, and `pluginStateVersion` for the plugin's schema. A save
  writes the higher of the block's and the build's for each, and keeps the keys
  it does not read, so a block a newer build wrote reaches it again whole
  ([the saved block's two versions][versions], [upgrading an older
  block][migration]).
- A parameter made at run time is in no set: no host lists it or hears of
  it, and a save writes it under its path ([parameters made at
  run time][run-time]).
- A tick's cost is what changed, never how many parameters made at run time
  exist: learn walks the armed and mapped ones, undo the gestures that
  closed, and the save's list of them is swept as new names double it.
- The audio thread never lets a store go. A typed parameter's drop asserts it
  is off the audio thread in a debug build, and a learn table that routes to
  a store drops on the UI thread.
- Where a host cuts its blocks never changes what a plugin hears. A stage that
  carries MIDI from one block to the next sends each message at its own frame
  and keeps nothing back for a block's end, so a stream cut anywhere gives the
  same messages at the same frames as one block does
  ([`streams.rs`](crates/ump/tests/streams.rs) checks it for the upgrade).
- Generated files (the page's protocol types, its test parameter map, `hapto.h`,
  the example EQ's JUCE settings) are committed and compared byte for byte in
  CI ([what runs the generator][generator], [keeping the header true][header]).
- A page in a plugin's window exists on Windows only: elsewhere
  `hapto-webview` offers no editor, and a host shows its own controls ([where
  the WebView pieces live][webview]). So does the standalone app, which says
  so and ends elsewhere ([the standalone exporter and app][standalone]).

## Threads

- [Main](docs/glossary.md#main-thread): owns the plugin, builds the `Processor`
  at activation, with learn's audio side when the parts have learn on, and
  takes both back at deactivation. An activation ticks the parts first, so a
  load the host made just before it is applied when the plugin builds, and
  plays from the first block. A tick, about 60 times a second, ticks the
  parts the plugin keeps, then calls `Plugin::idle`, then tells the host of
  the plugin's [edits](docs/glossary.md#edit) and of state it does not list
  that changed, then ticks each open editor
  ([a plugin's editor][editor]); no editor runs a timer of its own. A page's
  messages arrive on this thread too, from its message loop, outside any
  tick. A session
  restore applies to its participants in the parts' tick; `Driver::tick`
  (`parts.rs`) says what it ticks, and in what order. The engine reaches the
  parts through a `Driver`, which does not keep them alive, so a plugin that
  drops its parts is not ticked. The JUCE adapter ticks through JUCE on
  every platform, and the standalone app from its window's event loop. The
  VST3 exporter runs its own timer on Windows and macOS, and refuses to build
  for any other platform. There nothing would tick it: no edit
  would reach the host, and a load would never apply session sections to their
  participants ([what ticks a plugin][tick], [when a restore
  applies][applies]).
- Audio: owns the processor and learn's audio side, and never waits. A host
  write may arrive here, and so may the C interface's `get`, which the JUCE
  adapter's parameter reads call there ([the C interface][cabi]). The audio
  thread reads parameter values straight from the set's atomic cells.
- Any but audio: the host's save and load, and its text and query calls. Save
  and load go through `SavedState`. Text and query calls read the declarations,
  the parameter store, and the latency, tail and layout the main thread last
  read. None touches the plugin ([whether a plugin must be Send][send]).
- Which thread is main: VST3 builds the plugin at `initialize`, on the thread
  that calls it, which stays its main thread. The C interface keeps a plugin
  that is not `Send` on the thread that called `create`, and lends one that is
  `Send` to each main-thread call in turn, on any thread (a
  [loan](docs/glossary.md#loan)). The state is `MainThread` (`boundary.rs`),
  bound to one thread or lent call by call. Lent state holds nothing that is
  not `Send`: such a plugin keeps no parts, and the engine keeps no driver and
  no timer for it. The rule in full is on `Engine`
  (`engine/mod.rs`) for Hapto's code, on `Plugin` (`plugin.rs`) for plugin
  authors, and on `HaptoPlugin` in `hapto.h` for C users ([whether a plugin must
  be Send][send], [the VST3 exporter's shape][vst3], [the C interface][cabi]).

## Decided, not built

- The page path beyond Windows: wry inside a plugin window on macOS, and
  the JUCE adapter's own editor, which brings the page to AU hosts. Open: that
  macOS WebKit inside a host process loads plain `http://127.0.0.1` is not
  verified.
- MIDI-CI: a VST3 host speaks it for the plugin, which states what it supports
  through VST3's interfaces. Everywhere else a core crate, `hapto-ci`, answers
  with UMP in and out, SysEx7 included, through the C interface and CLAP. Open:
  the SysEx manufacturer ID its identity needs.
- Learn in VST3 through MIDI 2.0: the proxy grid of 16 channels by 130, built
  and listed through `IMidiMapping`, is to be listed through `IMidiMapping2`
  too, since Cubase asks only the second when a plugin has it, with a pool of
  proxies for MIDI 2.0 assignable controllers bound through it. The `vst3`
  crate's `Midi2Controller` is four bytes where the SDK's is two, so the
  exporter must define its own.
- Learn into a plain value: its target writes with `Origin::Learn` and opens
  no gesture, so a learned value is never an undo step and the target's
  `held` is `Value::gesture_open`. Hapto has no ready-made target for one.
- Handing the audio half of an owner of parameters made at run time to the
  processor and back. It keys an owner by its stores, not the plugin's id, and
  follows the plugin's list, so undo, redo and a load reach the audio side. A
  load's change to the tree reaches the processor as one message, the old
  halves out and the new in at the top of a block, and a load never assigns a
  parameter of an owner on its way out.
- A page names a parameter made at run time by its path; the generated
  parameter map stays the listed parameters'.
- CLAP: an exporter over the same engine, ticked through the host's
  timer-support extension. Open: a CLAP host may lack timer support.
- Learn's snapshot fallback: should a later `triple_buffer` break what the
  snapshot rests on, the fallback is a hand-written table with a reader count
  and `SeqCst` fences. Its `unsafe` is under the boundary's rules, with a miri
  job.

[applies]: docs/decisions.md#d13-when-a-session-restore-applies
[cabi]: docs/decisions.md#d62-the-c-interface-for-the-juce-adapter
[clock]: docs/decisions.md#d52-who-makes-the-restoreclock
[editor]: docs/decisions.md#d100-a-plugins-editor-and-its-tick
[faults]: docs/decisions.md#d75-making-a-caught-fault-visible
[generator]: docs/decisions.md#d27-what-runs-the-generator
[header]: docs/decisions.md#d63-keeping-the-c-header-true
[load]: docs/decisions.md#d39-a-load-between-a-replays-check-and-its-write
[mark]: docs/decisions.md#d67-checking-the-audio-thread-locking-rule
[migration]: docs/decisions.md#d80-upgrading-an-older-saved-block
[panic]: docs/decisions.md#d60-a-panic-inside-a-hosts-call
[run-time]: docs/decisions.md#d96-parameters-made-at-run-time
[send]: docs/decisions.md#d51-whether-a-plugin-must-be-send
[tick]: docs/decisions.md#d53-what-ticks-a-plugin-with-no-editor-open
[unlisted]: docs/decisions.md#d108-the-host-is-told-of-state-it-does-not-list
[versions]: docs/decisions.md#d84-the-saved-blocks-two-versions
[vst3]: docs/decisions.md#d59-the-shape-of-the-vst3-exporter
[webview]: docs/decisions.md#d16-where-the-webview-pieces-live
[standalone]: docs/decisions.md#d110-the-standalone-exporter-and-app
