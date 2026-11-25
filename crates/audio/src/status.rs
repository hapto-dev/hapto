//! A block's process status.

/// The status that a block's processing returns, which the host uses to
/// decide whether to keep calling the plugin.
///
/// It is CLAP's process status. VST3 has no equivalent, so its exporter
/// ignores it.
///
/// Open: no built exporter acts on it. The C interface returns it from
/// `process`, and the JUCE adapter, which calls that, ignores it. A CLAP
/// exporter is planned but not built.
///
/// # Examples
///
/// ```
/// use hapto_audio::Status;
///
/// // A processor with nothing left to do lets the host stop calling it.
/// fn after(block_was_silent: bool) -> Status {
///     if block_was_silent { Status::Sleep } else { Status::Continue }
/// }
/// assert_eq!(after(true), Status::Sleep);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Keep processing, whatever the input.
    Continue,
    /// Keep processing while the input or the output is not silent.
    ContinueIfNotQuiet,
    /// Keep processing for the plugin's tail once the input is silent.
    Tail,
    /// Nothing to do until input or events arrive again.
    Sleep,
}
