# Changelog

All notable changes to Hapto are recorded here. The format follows [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic
Versioning](https://semver.org/spec/v2.0.0.html): before 1.0, a minor version
may break the API.

## [Unreleased]

Nothing is released yet. Not built: the CLAP exporter, a page in a plugin
window anywhere but Windows, and MIDI delivered to a plugin through an
exporter.

### Changed

- The project was named Seam until this release, and is now Hapto: every
  crate, Rust path, C interface name, CMake function and variable, environment
  variable, the manifest's `[package.metadata.hapto]` table and the page
  package, now `@hapto/page`. The saved block's frame key is now
  `haptoStateVersion`, so a block saved before the rename is refused as one
  with no version.
- **Breaking.** A processor names its main bus's channels as a type,
  `Processor::Layout`: a `Bus<IN, OUT>` of up to 16 channels each way, with
  `Mono`, `Stereo`, `StereoOut`, `MonoToStereo` and `NoAudio` for the common
  ones, or `AnyLayout` for one that runs at each width its plugin lists,
  none wider than 16 channels, where the C interface took any. Its `Block`
  lends them as `Audio`, walked with `frames_mut` and `channels_mut`, with no
  index and no frame count, so `Block::channels` and `Block::frames` are
  gone. In a `Bus` both are arrays: `let [left, right] =
  block.audio.channels_mut()` reaches each channel by its place. The `Layout`
  struct is `layout::Width`, and `manifest_info!` takes `layouts` only for a
  processor in `AnyLayout`. The audio types are in a new core crate,
  `hapto-audio`, which `hapto-host` re-exports, with `AudioBuf`, audio of its
  own for tests.
- **Breaking.** `ProcessSetup` is made with `ProcessSetup::new`, with room for
  no MIDI until a test sets `max_events` or `max_out_events`, so the buses
  beyond the main one can join it later without breaking a caller.
- **Breaking.** MIDI is MIDI 2.0 inside. `ChannelVoice` reads and writes MIDI
  2.0's channel voice messages alone, a `ChannelMessage` for each status MIDI
  2.0 has, and `Voice` with its MIDI 1.0 variants is gone. An exporter runs
  `Upgrade` over each block, by the specification's Default Translation, and
  `Midi1Voice` reads and writes the MIDI 1.0 messages it translates;
  `Downgrade` translates back for a destination that takes MIDI 1.0 alone.
  Each `Event` keeps its `Origin`, and `scale` moves values between the
  widths. Learn reads MIDI 2.0 alone: a MIDI 1.0 controller learns and plays
  as before once upgraded, but for controllers 0 and 32, which select a bank.

### Added

- `hapto-params`: parameters declared once and checked by a builder, with one
  atomic store per parameter in real units. Every write snaps to the parameter's
  kind, never stores a non-finite value, and names its origin, which alone
  decides whether the host is told, a gesture opens, undo records and surfaces
  show it. Gestures nest and count, a program write is an edit or a sync, and
  change counters, watches and the host event drain carry changes. A parameter's
  value and origin share one atomic word, and `ParamSet::read` and
  `Value::read` return a `Reading`: the count, the value and the origin taken
  together, as `AudioParams` does on the audio thread. Plain values and lists
  hold state that is not a parameter.
- `hapto-params`: typed parameters. A `Float`, a `Bool` or a `Choice<E>`
  carries its store and reads with `get`, on any thread, as an `f32`, a `bool`
  or the plugin's own enum. `ParamSet::bind` makes one by key and checks it
  against its declaration once, refusing a missing key, another kind or a
  choice whose labels are not the enum's. `UiParams::write`, `begin`,
  `is_open`, `shown`, `would_change` and `Edit::write` take a typed parameter
  with a value of its type, or a `ParamId` with an `f32`; `Gesture::id` is
  `None` for a parameter made at run time. `Changes` asks once per block
  whether any of the parameters it is given moved.
- `hapto-params`: parameters declared where they are held. A struct
  implements `Params`, whose `declare` adds each parameter through a
  `Declarer`; a typed declaration, `FloatDecl`, `BoolDecl` or `ChoiceDecl<E>`,
  hands back its kind of parameter, so a field of another kind does not
  compile. `checked`, a `const fn`, refuses a bad key, group, choice labels,
  range or default while compiling. `ParamSet::declare` builds the set and
  returns the first declaration it refused, such as a key two parameters
  share. A typed declaration refuses a default outside its range, where a
  `ParamDecl` snaps it. `Declarer::extend` declares a plain table.
- `hapto-params`: `#[derive(Params)]`, under the `derive` feature, writes a
  struct's `declare` from a `#[param(...)]` mark on each field: the
  declaration's arguments, then `name = value` options and lone-word flags,
  with the kind taken from the field's type through `Declared`. A field with
  no mark declares a struct of parameters. Each declaration is checked while
  compiling, and so is a key two fields of the struct share: `cargo check`
  reports it on the mark. The example gain derives its parameters.
- `hapto-params`: `#[derive(Choices)]`, under the same feature, writes a
  choice's enum: its labels and both mappings, in the order of the variants.
  A label is the variant's name unless a `#[choice("...")]` mark gives one. A
  written discriminant must be the variant's position, and a label no choice
  could have is refused: `cargo check` reports either on the author's line.
  The example gain's `Mode` derives it.
- `hapto-undo`: an undo history that polls once per tick. One gesture is one
  step, transactions group steps, and host automation, restores and learned
  writes never record. A restore clears the history, and the history is bounded
  by the bytes its steps keep alive. `undo`, `redo` and `jump` return false
  when there is nothing to replay, or when a restore stops the walk, which
  clears the history. Enrolling a value or a list does not tick, unlike
  enrolling a parameter, so another value's writes on either side of it
  coalesce into one step.
- `hapto-session`: saved state beyond parameters, one keyed section per
  participant. A restore stores every section at once and applies on a later
  tick; a participant whose apply fails or panics keeps its saved section, and
  sections no participant claims are saved back. A participant held up by its
  own borrow is captured at the next tick, and one whose apply panicked is
  faulted until an apply succeeds. A capture nested too deep, or too large for
  the saved block to load back, is refused and the previous one kept, whether
  the participant registered before the saved state was made or after.
- `hapto-learn`: MIDI learn over MIDI 2.0 packets, with MIDI 1.0 upgraded to
  them. Arm a target, move a controller, and the mapping drives the target from
  the audio thread through a published snapshot that never waits, locks or
  frees. Notes, NRPNs, 14-bit controller pairs, curves, reservations and a saved
  `midiLearn` section. Half of a 14-bit pair waits at least a UI tick and a
  whole audio block for its other half. `Mapping` and `Config` are
  `non_exhaustive`, built with `Mapping::new`, `with_curve` and `as_coarse`.
  `arm`, `map`, `unmap`, `mapping` and `key` take anything `Learnable`: a
  `TargetId`, a `ParamId`, a `CustomId` or a typed parameter. A parameter made
  at run time is a target of its own, `TargetId::RunTime`, named by an opaque
  `RunTimeId`.
- `hapto-ump`: Universal MIDI Packets: a fixed-capacity event list per block,
  the channel voice messages read from it and written back, and the
  translation between MIDI 1.0 and MIDI 2.0.
- `hapto-host`: the `Plugin` and `Processor` traits and `PluginInfo`, and
  `SavedState`, the saved block as JSON, which names Hapto's frame version and
  the plugin's own schema version apart. A load keeps unknown parameter ids and
  top-level keys, migrates a block of an older plugin version or fails with the
  migration's reason, and refuses what is past fixed limits, a block that would
  save back past the size limit included; a setup past the block size limit is
  built for the limit. A plugin that is `Send` is lent to each main-thread call
  in turn, and a call that meets another thread's loan waits a bounded time
  before it is refused.
- `hapto-host`: Hapto builds each instance's parts. `Plugin::Params` names the
  struct that declares the parameters a host is shown; an exporter declares it,
  builds the set, the session and the saved state over it, and hands
  `Plugin::new` the parameters and the `Parts`, whose `ui` lends the handle an
  editor writes parameters through. A declaration refused fails the
  instance, with the reason in the fault report. `Parts::undo` and
  `Parts::midi_learn` turn undo and learn on, and return no `Result`: a
  mistake in the plugin's source, such as either turned on twice, panics with
  what to change, which refuses the instance. While a plugin keeps its parts,
  Hapto ticks them and the session before each `Plugin::idle`, which holds only
  the plugin's own work; a plugin never ticks them. It ticks them before each
  activation too, so a project a host loads and plays at once, with no tick
  between, plays its saved session from the first block. With learn on, each
  activation takes learn's audio side and each block runs it over the block's
  MIDI before the processor, so a processor holds no part of learn and never
  calls it. A plugin declares its saved-state migration as
  `Plugin::SCHEMA_VERSION` and `Plugin::migrate`. `Parts::for_plugin` builds a
  plugin's parts with no host, for tests, which tick them and take learn's
  audio side through a `Driver`.
- `hapto-host`: a plain value saved with the session in one call.
  `Parts::saved_value` makes a value of any type serde can write and read,
  starting at a default, and keeps it saved under a key, as JSON; a project
  saved without the key loads the same default. A
  change is captured at the next tick, or at once by `Parts::sync`, and a load
  is captured once, by its apply.
- `hapto-host`: parameters made at run time. `Parts::declare` and `Parts::add`,
  and a `Maker` for a session participant, make a `Float`, `Bool` or
  `Choice<E>` while the plugin runs, under a scope, from the same declarations
  as listed ones: listed to no host, written through `Parts::ui`, undone and
  learned as listed ones are, and read by the audio thread in the block after
  a write. The saved block keeps them under `runTime`, by path, each with its
  learned mapping. A load writes none of them on the host's thread: one
  declared under a path it holds is born holding the saved value. Once the
  load's pass has finished, each one still held takes the load's mapping for
  its path, and one made before the load its value too. Two under one path,
  and a save with no room for them, are faults.
- `hapto-host`: `manifest_info!`, under the `manifest` feature, writes a
  plugin's `PluginInfo` from its manifest. `[package.metadata.hapto]` gives who
  the plugin is: its name, vendor, addresses, id, VST3 class id as a UUID, AU
  codes, category and features. The package's version and description are the
  plugin's, and the layouts and MIDI flags are written in Rust, as the macro's
  fields. It checks the table while compiling, each mistake reported on its
  line in the manifest, and refuses a field written in both places. Both
  examples declare theirs this way.
- The VST3 exporter (`export_vst3!`), for Windows and macOS, with its own
  main-thread timer.
- The C interface (`export_cabi!`) and its generated header, `include/hapto.h`,
  which asserts every struct's layout as Rust laid it out.
- One exporter core under both. The core catches every panic at the boundary,
  counts and reports faults, and flushes denormals around each block. It stages
  each piece of a block's channels in place, and a model of every way a host may
  hand its buffers checks that. Test hosts drive a plugin through either
  exporter, and answer a refused call with what the host was told.
- `hapto-host`: the host is told when state it does not list changed, so it
  counts the project as changed: a saved value, a session participant's
  section, a learned mapping, or an edit to a parameter made at run time. A
  tick tells it once, after the change is captured, and never for a load. The
  VST3 exporter calls `IComponentHandler2::setDirty`, which its test host
  records as `HostCall::Dirty`. The C interface passes `HAPTO_RESTART_STATE`
  to the restart callback, in a call of its own, and `HAPTO_ABI_VERSION` is
  now 2. The JUCE adapter passes it on as a change to state that is no
  parameter.
- `hapto-host`: a plugin's editor. `Plugin::editor` hands out an `Editor`,
  `None` by default, which the VST3 exporter opens in the host's window on
  Windows, as a resizable view that follows the screen's scale; elsewhere it
  offers the host no view, and the host shows its own controls. The instance's
  timer ticks an open editor after `Plugin::idle`. An editor that fails to open
  is a fault with its reason, a panic in its `open`, its tick or its drop is a
  fault, an editor whose tick panicked is closed, and the host and the audio go
  on.
  `vst3::TestView` drives a view in tests, and on Windows attaches it to a
  `vst3::HiddenWindow`, which gives a real editor a window that is never shown
  and says what the editor left in it.
- `hapto-host`: on Windows, once an editor has opened, the plugin's module stays
  loaded until the process ends, so what the editor left with the host stays
  valid when the host unloads the module and loads it again. The plugin's file
  stays in use until the host exits.
- `hapto-bridge`: the page protocol, versioned, with its TypeScript generated
  from the Rust types and a check that the version moves when a message's shape
  does. Decoding refuses a position outside 0 to 1, from either side.
- `hapto-bridge`: the native side of the page protocol, `Dispatcher`, which a
  shell hands each message the page sends and ticks with the plugin. It answers
  every `ready`, `set`, `gestureEnd` and `setValue`, writes the page's drags
  through `UiParams` as a user's, stores the page's writes to the values a
  plugin binds in `Values` as a user's, runs the commands a plugin adds to
  `Commands`, tells the page of every change to a parameter or a value at the
  tick, and drops and counts what it cannot read (`Refusals`). `end_session`
  ends a page session when the page goes away and the dispatcher stays.
- `hapto-bridge`: values a page shows and writes, state that is not a
  parameter. A plugin binds a `Value<T>` under a key with `Values::value`; the
  snapshot carries it, a `value` message brings each change whatever made it,
  and a page's `setValue` that decodes to `T` is stored as the user's write,
  so undo records it.
- `hapto-webview`: a plugin's page in the host's window, on Windows. `Page`
  makes the editor from the page's files built into the plugin, a size, the
  values the page shows and writes (`Page::value`) and the commands the page may
  call: a WebView2 child window, through wry 0.57.2, which serves the files over
  a scheme of its own, keeps its data in the plugin's own folder below the
  user's local application data, and joins the page to a `Dispatcher`. A debug
  build loads `HAPTO_PAGE_DEV_URL` when it is set, with or without the files
  built in; with neither, `into_editor` gives `None`. The WebView stays on the
  page: it navigates nowhere else, takes no message from another document, and
  opens no file dropped on it. One that fails to start leaves nothing shown in
  the host's window. A command may run a dialog: the page's messages keep their
  order around it. Elsewhere `into_editor` gives `None`.
- `page`: `webviewTransport`, the transport of a page in a plugin's window.
- `page` and `hapto-webview`: the keyboard goes back to the host. After a
  click, a drag or a blur on a page in a plugin's window, the host's window
  gets the keyboard again unless a field, editable content, a `select` until
  a pick or a control marked `data-hapto-keys` has the focus, so the space
  bar reaches the host's transport.
- `page`: React hooks (`useParam`, `useValue`, `useEvent`, `useCommand`) over
  a pluggable transport, typed by the page's four maps. A held queue keeps the
  writes made before the native side answers and sends them in the order the
  page made them. A native message of the wrong shape fails the client
  visibly.
- `adapters/juce`: a generic JUCE adapter that builds any Hapto plugin as a JUCE
  plugin, through `hapto.h` and `hapto_add_juce_plugin`, for the formats JUCE
  builds. Checked as VST3 on Windows; CI builds VST3 and AU on macOS.
- `examples/gain`: a gain in dB with a bypass, a mode of three names, an
  undoable label saved with the plugin, and a gain MIDI learn can drive, as a
  VST3 module. Its page, a Vite and React app, shows a slider on the gain, a
  field for the label, which shows a load or an undo at once and drops an
  edit on Escape, and the version, in the host's window on Windows, when it
  was built in. It runs standalone too, under `cargo hapto dev`.
  [docs/first-plugin.md](docs/first-plugin.md) walks through it.
- `examples/eq`: an EQ of six biquad bands and an output gain, as a VST3 module
  or a static library for the adapter.
- `xtask`: `cargo xtask gen` writes every generated file, and CI fails when one
  drifts. `cargo xtask wrap` holds comments and docs to 80 columns.
- `cargo-hapto`: the plugin author's command, for a plugin crate in any
  workspace. `cargo hapto init` writes a new plugin crate, a gain with its
  page, a standalone binary and tests, with fresh ids, its Hapto crates from
  a checkout. `cargo hapto dev` runs the plugin's standalone binary with its
  page from Vite, and rebuilds and restarts it after each change to Rust with
  its saved block kept; `--host` runs it in the dev host. `cargo hapto serve`
  runs the page's dev server alone, for the debug bundle in a real host.
  `cargo hapto build` writes a plugin crate's page map, builds its page and
  the crate in release, lays out its VST3 bundle under `target/bundle/` and
  installs it, per user or for every user, on Windows. `cargo hapto validate`
  runs pluginval on that bundle as CI does: CI's validator lane runs it. Each
  command's `--help` says its flags.
- `hapto-host`: the standalone exporter, behind the `standalone` feature: a
  plugin driven by an app of its own with no host, through the same engine.
  With the feature, a fault's line goes to stderr on Windows too, the app's
  terminal.
- `hapto-standalone`: a plugin as an app of its own, on Windows, which a plugin
  crate's binary runs: its editor in a window, a test signal through it to the
  system's audio output, which it follows to a new default output, and its
  saved block kept between runs. Its title counts the plugin's faults. Its dev
  host, `cargo hapto dev --host`, runs the plugin through the VST3 exporter or
  the C interface in process, in a window of the plugin's own with a bar that
  names the format, and a settings window of what the host sees, a tab each,
  which switches between the two formats keeping the saved block. It changes the
  audio as a host does: the sample rate, of those the device takes, and the most
  frames a block holds, each by reactivating the plugin, blocks cut as the
  device asks, at the maximum, at random or at awkward lengths, and the test
  signal; it stops and starts processing. It lists the parameters as the host
  does, each with its kind and the MIDI learn mappings that name it, logs what
  the plugin tells the host and what the dev host does to the plugin, and shows
  the saved block and how its first load went. A block the plugin refused is
  kept for the next run rather than saved over.
- `hapto-host`: `vst3::TestView::attach_to` attaches a test host's view to a
  window of the caller's, and `vst3::ChildWindow` is a window inside one of
  the caller's for it, placed where a host wants the view.
  `vst3::HiddenWindow` gives its window handle.
- `bench`: the comparison of the example EQ against the same EQ in plain JUCE,
  with its reports in `bench/reports`.
- Guarantees the tests keep: the audio path never allocates (allocation gates),
  the lock-free hand-offs pass `loom` models, `unsafe` lives only in the
  exporters' boundary modules, the core crates build with no window, WebView or
  plugin-format dependency, and fuzz targets cover the UMP reader, the page
  messages, the saved block and the C interface's state.
