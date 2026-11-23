//! A plugin as an app of its own: [`main`] runs any [`Plugin`] standalone,
//! its editor in a window and its output on the system's audio output,
//! through Hapto's standalone exporter. Its inputs hear silence unless the
//! command line asks for a test signal.
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
//! `src/bin/standalone.rs` is one call, [`main`]. `cargo hapto dev` builds the
//! binary and runs it with `--dev`, restarts it after each rebuild with the
//! plugin's saved block kept, and serves the page from its dev server. Run on
//! its own, the app keeps the saved block in the plugin's folder below the
//! user's local application data. No MIDI reaches the plugin.
//!
//! With the `dev-host` feature, `--host` runs the plugin through the VST3
//! exporter or the C interface instead, in process, by Hapto's test hosts,
//! with a settings window of what the host sees: `cargo hapto dev --host`.
//!
//! Windows only for now, as the page in a window is: elsewhere [`main`] says
//! so and ends.

// Each module but the app and the device is platform-neutral and tested
// everywhere, but only Windows runs them.
#[cfg(windows)]
mod app;
#[cfg(any(windows, test))]
mod block;
#[cfg(all(windows, feature = "dev-host"))]
mod dev_host;
#[cfg(windows)]
mod device;
// The audio paths, open to the crate's allocation gate alone.
#[cfg(any(windows, test))]
#[doc(hidden)]
pub mod gate;
#[cfg(any(windows, test))]
mod options;
#[cfg(any(windows, test))]
mod session;
#[cfg(any(windows, test))]
mod settings;
#[cfg(any(windows, test))]
mod signal;

use std::process::ExitCode;

use hapto_host::Plugin;

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
    run::<P>().unwrap_or_else(|reason| {
        eprintln!("{reason:#}");
        ExitCode::FAILURE
    })
}

#[cfg(windows)]
fn run<P: Plugin>() -> anyhow::Result<ExitCode> {
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
    app::run::<P>(&options)
}

#[cfg(not(windows))]
fn run<P: Plugin>() -> anyhow::Result<ExitCode> {
    Err(anyhow::anyhow!(
        "{}: the standalone app runs on Windows only for now",
        P::INFO.name
    ))
}
