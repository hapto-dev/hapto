//! A plugin as an app of its own: [`main`] runs any [`Plugin`] standalone,
//! its editor in a window and its output on the system's audio output,
//! through Hapto's standalone exporter. Its inputs hear silence unless the
//! command line asks for a test signal. A plugin that takes or sends MIDI
//! reaches the computer's MIDI devices as [`MidiDevices`] says, which the
//! binary sets through [`App`].
//!
//! A plugin crate gives it a binary behind a `standalone` feature, so the
//! plugin's module never links it:
//!
//! ```toml
//! [features]
//! standalone = ["dep:hapto-standalone"]
//!
//! [dependencies]
//! hapto-standalone = { version = "0.1", optional = true }
//!
//! [[bin]]
//! name = "my-plugin-standalone"
//! path = "src/bin/standalone.rs"
//! required-features = ["standalone"]
//! ```
//!
//! `src/bin/standalone.rs` is one call, [`main`], or an [`App`] built and
//! run. `cargo hapto dev` builds the binary and runs it with `--dev`,
//! restarts it after each rebuild with the plugin's saved block kept, and
//! serves the page from its dev server. Run on its own, the app keeps the
//! saved block in the plugin's folder below the user's local application
//! data.
//!
//! With the `dev-host` feature, `--host` runs the plugin through the VST3
//! exporter or the C interface instead, in process, by Hapto's test hosts,
//! with a settings window of what the host sees: `cargo hapto dev --host`.
//!
//! Windows only for now, as the page in a window is: elsewhere [`main`] says
//! so and ends.

// Each module but the app, the device, the MIDI devices and the dev host is
// platform-neutral and tested everywhere, but only Windows runs them.
#[cfg(windows)]
mod app;
#[cfg(any(windows, test))]
mod block;
#[cfg(all(windows, feature = "dev-host"))]
mod dev_host;
#[cfg(windows)]
mod device;
#[cfg(windows)]
mod midi;
// The audio paths, open to the crate's allocation gate alone.
#[cfg(any(windows, test))]
#[doc(hidden)]
pub mod gate;
#[cfg(any(windows, test))]
mod options;
mod routing;
#[cfg(any(windows, test))]
mod session;
#[cfg(any(windows, test))]
mod settings;
#[cfg(any(windows, test))]
mod signal;
#[cfg(test)]
mod tests;

use std::marker::PhantomData;
use std::process::ExitCode;

use hapto_host::Plugin;
pub use hapto_midi_devices::{Device, DeviceId, Direction, Timing};
pub use routing::MidiDevices;

/// Runs `P` standalone, as a binary's `main`, reading the command line, until
/// its window closes. Returns the exit code: failure, with the reason
/// written, when it could not run.
///
/// The command line takes `--signal sine|noise|silence`, what plays into
/// the plugin's inputs, silence by default; `--dev <folder>`,
/// which `cargo hapto dev` passes; `--host`, which runs the dev host, built
/// with the `dev-host` feature; and `--help`.
///
/// # Examples
///
/// A plugin crate's `src/bin/standalone.rs`:
///
/// ```no_run
/// # use hapto_host::layout::Stereo;
/// # use hapto_host::{
/// #     AuCodes, Block, Category, Parts, Plugin, PluginInfo, ProcessSetup, Processor, Status,
/// # };
/// # use hapto_params::{Declarer, Params};
/// # #[derive(Clone)]
/// # struct NoParams;
/// # impl Params for NoParams {
/// #     fn declare(_: &mut Declarer) -> Self {
/// #         NoParams
/// #     }
/// # }
/// # struct Pass;
/// # impl Processor for Pass {
/// #     type Layout = Stereo;
/// #     fn process(&mut self, _: Block<'_, Stereo>) -> Status {
/// #         Status::Continue
/// #     }
/// # }
/// # struct MyPlugin;
/// # impl Plugin for MyPlugin {
/// #     const INFO: PluginInfo = PluginInfo {
/// #         name: "My Plugin", vendor: "Me", url: "", email: "", description: "",
/// #         version: "0.1.0", id: "com.example.my-plugin", vst3_class_id: [0; 16],
/// #         au_codes: AuCodes { subtype: *b"mypl", manufacturer: *b"Exmp" },
/// #         category: Category::Effect, features: &[], layouts: &[],
/// #         midi_input: false, midi_output: false,
/// #     };
/// #     type Params = NoParams;
/// #     type Processor = Pass;
/// #     fn new(_: NoParams, _: Parts) -> Self {
/// #         MyPlugin
/// #     }
/// #     fn activate(&mut self, _: &ProcessSetup) -> Pass {
/// #         Pass
/// #     }
/// # }
/// fn main() -> std::process::ExitCode {
///     hapto_standalone::main::<MyPlugin>()
/// }
/// ```
pub fn main<P: Plugin>() -> ExitCode {
    App::<P>::new().run()
}

/// `P` as an app, with the MIDI devices it opens said in code: a filter for
/// each direction, and when the outputs send, by the rules and with the
/// defaults of [`MidiDevices`]. [`midi`](Self::midi) is the handle that
/// changes them while the app runs. [`main`] is `App::<P>::new().run()`.
///
/// # Examples
///
/// A plugin crate's `src/bin/standalone.rs`, which hears one keyboard and
/// sends to a virtual port:
///
/// ```no_run
/// # use hapto_host::layout::Stereo;
/// # use hapto_host::{
/// #     AuCodes, Block, Category, Parts, Plugin, PluginInfo, ProcessSetup, Processor, Status,
/// # };
/// # use hapto_params::{Declarer, Params};
/// # #[derive(Clone)]
/// # struct NoParams;
/// # impl Params for NoParams {
/// #     fn declare(_: &mut Declarer) -> Self {
/// #         NoParams
/// #     }
/// # }
/// # struct Pass;
/// # impl Processor for Pass {
/// #     type Layout = Stereo;
/// #     fn process(&mut self, _: Block<'_, Stereo>) -> Status {
/// #         Status::Continue
/// #     }
/// # }
/// # struct MyPlugin;
/// # impl Plugin for MyPlugin {
/// #     const INFO: PluginInfo = PluginInfo {
/// #         name: "My Plugin", vendor: "Me", url: "", email: "", description: "",
/// #         version: "0.1.0", id: "com.example.my-plugin", vst3_class_id: [0; 16],
/// #         au_codes: AuCodes { subtype: *b"mypl", manufacturer: *b"Exmp" },
/// #         category: Category::Effect, features: &[], layouts: &[],
/// #         midi_input: true, midi_output: true,
/// #     };
/// #     type Params = NoParams;
/// #     type Processor = Pass;
/// #     fn new(_: NoParams, _: Parts) -> Self {
/// #         MyPlugin
/// #     }
/// #     fn activate(&mut self, _: &ProcessSetup) -> Pass {
/// #         Pass
/// #     }
/// # }
/// use std::sync::OnceLock;
///
/// use hapto_standalone::MidiDevices;
///
/// /// The app's MIDI devices, where the plugin's own code reaches them:
/// /// its page, say, which lists them and lets a user pick.
/// static MIDI: OnceLock<MidiDevices> = OnceLock::new();
///
/// fn main() -> std::process::ExitCode {
///     let app = hapto_standalone::App::<MyPlugin>::new()
///         .with_midi_inputs(|device| device.name().contains("Xkey"))
///         .with_midi_outputs(|device| device.name().starts_with("loopMIDI"))
///         // An arpeggiator: its notes keep their spacing.
///         .with_midi_out_timing(hapto_standalone::Timing::AtFrames);
///     let _ = MIDI.set(app.midi());
///     app.run()
/// }
/// ```
pub struct App<P: Plugin> {
    midi: MidiDevices,
    plugin: PhantomData<fn() -> P>,
}

impl<P: Plugin> std::fmt::Debug for App<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("plugin", &P::INFO.name)
            .field("midi", &self.midi)
            .finish()
    }
}

impl<P: Plugin> Default for App<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Plugin> App<P> {
    /// `P` as an app with the MIDI devices [`MidiDevices`] opens by
    /// default.
    pub fn new() -> Self {
        App {
            midi: MidiDevices::new(),
            plugin: PhantomData,
        }
    }

    /// Opens the MIDI inputs `filter` passes, as
    /// [`MidiDevices::set_inputs`] does.
    #[must_use]
    pub fn with_midi_inputs(
        self,
        filter: impl Fn(&Device) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.midi.set_inputs(filter);
        self
    }

    /// Opens the MIDI outputs `filter` passes, as
    /// [`MidiDevices::set_outputs`] does.
    #[must_use]
    pub fn with_midi_outputs(
        self,
        filter: impl Fn(&Device) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.midi.set_outputs(filter);
        self
    }

    /// Has the outputs send as `timing` says, as
    /// [`MidiDevices::set_timing`] does.
    #[must_use]
    pub fn with_midi_out_timing(self, timing: Timing) -> Self {
        self.midi.set_timing(timing);
        self
    }

    /// The handle to the app's MIDI devices, to keep: it changes them while
    /// the app runs, as [`MidiDevices`] says. The plugin is made inside
    /// [`run`](Self::run), so its own code reaches the handle where the
    /// binary keeps it, in a `static`, as [`App`]'s example does.
    pub fn midi(&self) -> MidiDevices {
        self.midi.clone()
    }

    /// Runs the app, reading the command line, until its window closes.
    /// Returns the exit code: failure, with the reason written, when it
    /// could not run.
    pub fn run(self) -> ExitCode {
        run::<P>(self.midi).unwrap_or_else(|reason| {
            eprintln!("{reason:#}");
            ExitCode::FAILURE
        })
    }
}

#[cfg(windows)]
fn run<P: Plugin>(midi: MidiDevices) -> anyhow::Result<ExitCode> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", options::USAGE);
        return Ok(ExitCode::SUCCESS);
    }
    let options = options::Options::parse(args)?;
    if options.host {
        #[cfg(feature = "dev-host")]
        return dev_host::run::<P>(&options);
        #[cfg(not(feature = "dev-host"))]
        anyhow::bail!("--host needs hapto-standalone's dev-host feature");
    }
    app::run::<P>(&options, midi)
}

#[cfg(not(windows))]
fn run<P: Plugin>(_midi: MidiDevices) -> anyhow::Result<ExitCode> {
    Err(anyhow::anyhow!(
        "{}: the standalone app runs on Windows only for now",
        P::INFO.name
    ))
}
