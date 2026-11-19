//! The app's command line.

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use crate::signal::Signal;

/// What the app prints when its arguments are wrong.
pub(crate) const USAGE: &str =
    "usage: <app> [--signal sine|noise|silence] [--dev <folder>] [--host]

  --signal   what plays into the plugin's inputs: silence (the default), a 440 Hz
             sine at -24 dBFS, or white noise at -24 dBFS
  --dev      run under `cargo hapto dev`: keep the saved block in <folder>, save it
             each second it changed, and quit, saving, when stdin closes
  --host     run the plugin through the dev host: the VST3 exporter, or the C
             interface, in process, with a settings window; needs the dev-host feature";

/// What the app was asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Options {
    /// What plays into the plugin's inputs.
    pub(crate) signal: Signal,
    /// The dev session's folder, under `cargo hapto dev`.
    pub(crate) dev: Option<PathBuf>,
    /// Whether to run the plugin through the dev host.
    pub(crate) host: bool,
}

impl Options {
    /// Reads the app's arguments, its own name excluded.
    ///
    /// # Errors
    ///
    /// Returns what is wrong with them, and the usage.
    pub(crate) fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
        let mut options = Options {
            signal: Signal::Silence,
            dev: None,
            host: false,
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let mut value = || {
                args.next()
                    .ok_or_else(|| format!("{arg} wants a value\n\n{USAGE}"))
            };
            match arg.as_str() {
                "--signal" => {
                    let name = value()?;
                    options.signal = Signal::named(&name)
                        .ok_or_else(|| format!("no signal is named {name:?}\n\n{USAGE}"))?;
                }
                "--dev" => options.dev = Some(value()?.into()),
                "--host" => options.host = true,
                _ => return Err(format!("the app does not take {arg:?}\n\n{USAGE}")),
            }
        }
        Ok(options)
    }
}
