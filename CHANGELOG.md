# Changelog

All notable changes to Hapto are recorded here. The format follows [Keep a
Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic
Versioning](https://semver.org/spec/v2.0.0.html): before 1.0, a minor version
may break the API.

## [Unreleased]

Nothing is released yet. Not built: the CLAP exporter, and a page in a plugin
window anywhere but Windows.

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
  own for tests. Every layout's frame takes a mono voice's sample into each
  of its channels with `AddSample::add`. `Width` names the layouts' widths,
  `STEREO_OUT`, `MONO_TO_STEREO` and `NO_AUDIO` beside `MONO` and `STEREO`,
  and the C interface's test host's `activate` takes its frames as a `usize`,
  as a block counts them.
- **Breaking.** `ProcessSetup` is made with `ProcessSetup::new`, with room for
  no MIDI until a test sets `max_events` or `max_out_events`, so the buses
  beyond the main one can join it later without breaking a caller.
- **Breaking.** `StandaloneAudio::process` takes a `StandaloneBlock`, the
  block's channels, frames and MIDI in, and `sent`, which it hands what the
  plugin sent once the block has played, at the block's frames, for an app to
  send to its devices. `StandaloneBlock::new` makes a block of audio alone, as
  long as its shortest channel, and `with_midi` gives it MIDI and its length,
  so a plugin with no channels plays its MIDI.
- The VST3 exporter sends a plugin's MIDI to its host, for a plugin that
  declares MIDI out: notes as VST3's note events, under the host's `noteId`
  where the host named the note and none where the plugin made it up, a
  note's pressure, the note expressions of a note its host named, and a whole
  SysEx. A controller, channel pressure, the bend and a program change go out
  as MIDI 1.0 controllers, downgraded, and so do the system messages VST3
  numbers, from start and stop to song position. `MidiOut::send` keeps a
  host's note id in the packet's origin, and `midi_out.extend(midi)` is MIDI
  thru. The VST3 test host gains `sent`, `BusEvent::Controller` and
  `parameters`, which lists those a host shows, without the 2,080 hidden
  proxies for a plugin that takes MIDI.
- The VST3 exporter brings a host's MIDI in, for a plugin that takes MIDI:
  notes, a note's pressure and VST3's note expressions as MIDI 2.0, each note
  named by the host's `noteId`, and SysEx. A MIDI 1.0 controller, channel
  pressure and the pitch bend come through hidden proxy parameters, 16
  channels by 130, which `IMidiMapping` names; a host lists them after the
  plugin's own, and each shows its MIDI 1.0 value as text. `Origin::Note`
  carries a format's note id beside its packet, and the decoder names the note
  by it: its note-off ends that note alone. An exporter may hand the engine a
  block's MIDI in any order; the block takes it in time order. The VST3 test
  host gains `process_events`, `process_claiming`, `BusEvent`, `midi_mapping`,
  `note_expressions` and `ParamInfo::is_hidden`.
- **Breaking.** The C interface carries a block's MIDI as Universal MIDI
  Packets both ways, so `HAPTO_ABI_VERSION` is 3. `HaptoProcess` gains the
  packets in and the room for packets out, each a `HaptoPacket` of a frame and
  four words, with `HAPTO_MIDI_OUT_1` for a caller that takes MIDI 1.0 out
  alone, which then gets MIDI 1.0's packets alone, and `HaptoProcess::NONE`
  fills what a Rust caller leaves out. The JUCE adapter passes the host's
  `MidiBuffer` in and the plugin's MIDI back, in room it makes before the
  first block.
- **Breaking.** A processor reads its block's MIDI decoded: `block.midi` is
  a `Midi`, each `Message` at its frame with the packets it came in, and
  `block.midi_out` a `MidiOut`, which sends messages or packets, in place of
  `block.events` and `block.out_events`. `Block::new` takes the two, and
  `Midi::default()` is a block with no MIDI. The engine decodes every block
  once, and a block in pieces hands each piece its share. `hapto_host::midi`
  re-exports `hapto-midi`. `block.spans()` cuts a block into `Span`s at each
  frame a message arrives at, each with its audio and the messages at its
  first frame, and `Spans::at_most` caps their length;
  `block.spans_and_out()` hands the block's MIDI out back beside them, for a
  plugin that sends too. `Audio::split_at` splits audio at a
  frame.
- **Breaking.** MIDI is MIDI 2.0 inside. `ChannelVoice` reads and writes MIDI
  2.0's channel voice messages alone, a `ChannelMessage` for each status MIDI
  2.0 has, and `Voice` with its MIDI 1.0 variants is gone. MIDI 1.0 becomes
  MIDI 2.0 through `Upgrade`, by the specification's Default Translation, and
  `Midi1Voice` reads and writes the MIDI 1.0 messages it translates;
  `Downgrade` translates back for a destination that takes MIDI 1.0 alone,
  passing only what MIDI 1.0 carries and returning how many packets it
  dropped. Each `Event` keeps its `Origin`, and `scale` moves values between
  the widths. An RPN or NRPN's data entry high byte goes out at its own frame,
  and its low byte joins it there or refines it later, so where a host cuts
  its blocks changes nothing. Controllers 0 and 32 pass on as controllers as
  well as riding the next program change, so a fader on them learns, and MIDI
  1.0 sent back through `Downgrade` repeats its bank select before the
  program. Learn reads MIDI 2.0 alone: a MIDI 1.0 controller learns and plays
  as before once upgraded, but for four rules of NRPN data entry. A low byte
  refines its number's high byte when the address is sent again between them,
  where learn dropped the entry; an address byte sent alone keeps the other
  one held, as a MIDI 1.0 receiver does, where learn forgot the low byte at a
  lone 99; a binding that is not coarse skips a high byte sent alone and moves
  when its low byte comes, so a pair a block cuts never dips; and a coarse
  binding reads the high byte of a data entry that carries its low byte too,
  where learn wrote nothing. A packet's message type is a `MessageType`, whose
  constants name the UMP table's types and whose `words` replaces
  `packet_words`.
- `hapto-standalone` keeps its session in the `Standalone` folder of the same
  data folder as the WebView's, so it names the folder by the WebView's rule:
  each character of the plugin's id other than an ASCII letter, a digit, `.`,
  `-` and `_` is written as `_`. A session saved for an id with such a
  character is not found again. An id that `cargo hapto init` writes keeps
  every character. An empty `LOCALAPPDATA` now counts as unset, so the app
  fails as it does when the variable is unset.

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
- `hapto-midi`: MIDI in musicians' terms. `Decoder` reads a block's MIDI 2.0
  packets into `Message`s: notes with a `NoteId` from note-on to note-off,
  their own messages naming their `NoteKey`, controllers, bend and its range,
  pressure, programs with their bank, and SysEx joined whole. Values from 0
  to 1 are `f32`, values with a centre `Centred`, and `Key`, `Group`,
  `Channel` and `Program` show as musicians count them. Decoding allocates
  nothing; what overflows its tables is counted. A `Controller<T>` is read
  typed and sent: `controller::PAN` as `Centred`, `controller::SUSTAIN` as a
  `Pedal`, which keeps half pedal, and a plugin's own with a value type of
  its own by `ControllerValue`; `controller::number` keeps the plain
  numbers. `Controller::read_channel` reads a value with the group and
  channel it came on, for a `PerChannel`, which keeps a value for each of the
  256. `packets_of` turns messages into the packets they go out as, for a
  test or a tool to hand a host. `NoteOn::with_key` moves a note, its own
  pitch with it, and `NoteOn::with_id` with `NoteId::own` names a note a
  plugin sends beside the host's.
- `hapto-midi-devices`: the computer's MIDI devices as UMP, for an app that runs
  a plugin with no host. A device is named by an id that survives the system's
  renumbering, the list's changes are told at the main thread's tick, and each
  input open is read on the system's thread into a ring that never waits, locks
  or allocates, then into each block at the frames its messages came at. Every
  device is merged on group 1, as a host hands a plugin its MIDI, each upgraded
  to MIDI 2.0 with a translation of its own, and a SysEx reaches a block whole.
  `Outputs` opens devices to send to, and `OutputsWriter` hands each block's
  MIDI to them from the audio thread, downgraded to MIDI 1.0, sent by a thread
  of its output's own as soon as the block has played or, by `Timing`, each
  message at its frame's time. WinMM is its backend on Windows, and ALSA's
  sequencer on Linux, where a MIDI 2.0 device comes in as UMP.
- `hapto-ump`: `Upgrade::reset` forgets what an upgrade holds, in place, for a
  new source.
- `hapto-ump`: Universal MIDI Packets: a fixed-capacity event list per block,
  the channel voice messages and 7-bit SysEx read from it and written back,
  SysEx joined back into messages by the specification's rules,
  the translation between MIDI 1.0 and MIDI 2.0, and MIDI 1.0's byte stream
  read into packets and written back.
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
  edit on Escape, a Learn button that arms MIDI learn on the gain and says
  where learn is, and the version, in the host's window on Windows, when it
  was built in. It runs standalone too, under `cargo hapto dev`.
  [docs/first-plugin.md](docs/first-plugin.md) walks through it.
- `examples/eq`: an EQ of six biquad bands and an output gain, as a VST3 module
  or a static library for the adapter.
- `examples/midi-probe`: a plugin that makes a host's MIDI heard, for checks by
  hand: a sine for each note, which its own pitch, bend, pressure and volume
  move, the channel's bend and volume, and every message sent back out as it
  came. It builds as a VST3 module, through the JUCE adapter, and as an app of
  its own on the computer's MIDI devices.
- `xtask`: `cargo xtask gen` writes every generated file, and CI fails when one
  drifts. `cargo xtask wrap` holds comments and docs to 80 columns.
- `cargo-hapto`: the plugin author's command, for a plugin crate in any
  workspace. `cargo hapto init` writes a new plugin crate, a gain with its
  page, a standalone binary and tests, with fresh ids, its Hapto crates from
  a checkout; `--instrument` writes an instrument instead, a sine for each
  note with no page. `cargo hapto dev` runs the plugin's standalone binary
  with its page from Vite, and rebuilds and restarts it after each change to
  Rust with its saved block kept; `--host` runs it in the dev host. `cargo
  hapto serve` runs the page's dev server alone, for the debug bundle in a
  real host.
  `cargo hapto build` writes a plugin crate's page map, builds its page and
  the crate in release, lays out its VST3 bundle under `target/bundle/` and
  installs it, per user or for every user, on Windows. `cargo hapto validate`
  runs pluginval on that bundle as CI does: CI's validator lane runs it.
  `cargo hapto juce` writes the crate's settings for the JUCE adapter through
  its `juce_settings` test, which `init` writes with a `cabi` feature, and
  prints the CMake that builds it through JUCE. Each
  command's `--help` says its flags.
- `hapto-host`: the standalone exporter, behind the `standalone` feature: a
  plugin driven by an app of its own with no host, through the same engine.
  With the feature, a fault's line goes to stderr on Windows too, the app's
  terminal.
- `hapto-standalone`: a plugin as an app of its own, on Windows, which a plugin
  crate's binary runs: its editor in a window, a test signal through it to the
  system's audio output, which it follows to a new default output, and its
  saved block kept between runs. A plugin that takes MIDI hears every MIDI
  input the computer has, merged as a host merges them, opened and closed as
  devices come and go, and a line names each, and one that sends MIDI sends
  it to the outputs its binary chooses, as soon as each block has played or
  each message at its frame. `App` says in code which devices open, a filter
  for each direction, every input and no output by default, and when they
  send, and its `MidiDevices` handle changes them while the app runs, from
  any thread. Its title counts the plugin's faults. Its dev
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
- `hapto-host`: `vst3::TestView::attach` attaches a test host's view to any
  window of the caller's that gives a handle, and `vst3::ChildWindow` is a
  window inside one of the caller's for it, placed where a host wants the
  view. `vst3::HiddenWindow` gives its window handle. Both windows build on
  every system, and their `new` returns `None` on one that the test host makes
  no windows on.
- `bench`: the comparison of the example EQ against the same EQ in plain JUCE,
  with its reports in `bench/reports`.
- Guarantees the tests keep: the audio path never allocates (allocation gates),
  the lock-free hand-offs pass `loom` models, `unsafe` lives only in the
  exporters' boundary modules, the core crates build with no window, WebView or
  plugin-format dependency, and fuzz targets cover the UMP reader, the page
  messages, the saved block and the C interface's state.
