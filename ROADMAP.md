# Roadmap

Where Seam is going, in the order it means to get there. This is the big
picture only: each entry is a goal, not a task list. Working plans and tickets
stay out of the repository, and this file changes in a pull request of its own.

The README's [Status](README.md#status) says what is built today, and
ARCHITECTURE.md's [Decided, not built](ARCHITECTURE.md#decided-not-built) holds
the design of what is decided and waiting.

## Now

1. **The plugin author's API.** The shape a plugin is written against, made
   idiomatic Rust at the plugin boundary, before anything depends on it.
2. **The page in a plugin window.** A plugin's editor as a page in a WebView
   inside the host's window, on Windows first.
3. **A real host.** The example plugins, one with its page showing, probed in
   Cubase, which no validator stands in for. Passing it makes the first
   tagged release, `v0.1.0`.

## Next

- **macOS as a target.** A VST3 bundle for macOS, laid out and validated as
  the Windows one is, with the page in its window. The scheduled CI lanes
  return with it: the nightly and weekly workflows and the JUCE adapter's
  macOS build, which are started by hand until then.
- **The page in more formats.** The JUCE adapter shows a plugin's editor,
  which brings the page to AU hosts first. A standalone app and AAX follow.
- **MIDI through the exporters.** MIDI in and out for a plugin in a host, and
  with it MIDI learn in VST3.
- **More for the page.** The hooks the page's README lists as
  [not built yet](page/README.md#not-built-yet).

## Later

- **CLAP.** An exporter over the same engine as VST3 and the C interface.
- **MIDI-CI.** Spoken by the host for a VST3 plugin, and answered by Seam
  everywhere else.
- **A comparison with the editors open.** The bench run again with each
  build's UI showing, once a plugin window can show Seam's page.
- **The first public release.** The docs raised to reference grade.

Open: the order within Next and Later is not settled.
