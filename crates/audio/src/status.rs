//! What a block's processing tells the host about the next ones.

/// What a block's processing tells the host about the next ones.
///
/// It is CLAP's process status, which a host uses to stop calling a quiet
/// plugin. VST3 has no such answer, so its exporter ignores it.
///
/// Open: no built exporter acts on it. The C interface answers it from
/// `process`, and the JUCE adapter, which calls that, ignores it. A CLAP
/// exporter is planned, not built.
///
/// # Examples
///
/// ```
/// use hapto_audio::Status;
///
/// // A processor that has nothing left to do tells a host that may stop it.
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
