//! A command's arguments, read one at a time.

#[cfg(test)]
mod tests;

use std::num::NonZeroU16;

/// Whether `args`, a command's, ask for its usage: a `-h` or `--help`
/// anywhere among them.
pub(crate) fn wants_help(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "-h" || arg == "--help")
}

/// The arguments after a command's name, and the usage its errors end with.
#[derive(Debug)]
pub(crate) struct Args<I> {
    args: I,
    usage: &'static str,
}

impl<I: Iterator<Item = String>> Args<I> {
    /// Reads `args`, ending each error with `usage`.
    pub(crate) fn new(
        args: impl IntoIterator<Item = String, IntoIter = I>,
        usage: &'static str,
    ) -> Self {
        Args {
            args: args.into_iter(),
            usage,
        }
    }

    /// Returns the value after `flag`.
    ///
    /// # Errors
    ///
    /// Says `flag` wants one, when no argument follows.
    pub(crate) fn value(&mut self, flag: &str) -> Result<String, String> {
        self.args
            .next()
            .ok_or_else(|| format!("{flag} wants a value\n\n{}", self.usage))
    }

    /// Returns the port after `flag`: one to listen on, so not 0, which
    /// would leave the system to pick one and nothing to find it by.
    ///
    /// # Errors
    ///
    /// Says `flag` wants one, when no argument follows or it is no port.
    pub(crate) fn port(&mut self, flag: &str) -> Result<u16, String> {
        let port = self.value(flag)?;
        port.parse::<NonZeroU16>()
            .map(NonZeroU16::get)
            .map_err(|_| format!("{flag} wants a port from 1 to 65535, not {port:?}"))
    }

    /// Returns the error for an argument the command does not take.
    pub(crate) fn unknown(&self, arg: &str) -> String {
        format!("this command does not take {arg:?}\n\n{}", self.usage)
    }
}

impl<I: Iterator<Item = String>> Iterator for Args<I> {
    type Item = String;

    fn next(&mut self) -> Option<String> {
        self.args.next()
    }
}
