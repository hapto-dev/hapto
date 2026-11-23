//! What the audio thread is handed each block.

pub use hapto_ump::{Event, EventList};

use crate::Audio;
use crate::layout::{Layout, Width};
use crate::{Midi, MidiOut};

/// The most frames a processor is built for per block: 65,536, about 1.4
/// seconds at 48 kHz.
///
/// The processor's buffers are allocated up front, so the exporters build a
/// setup that promises more for this many, and a longer block plays in pieces.
pub const MAX_FRAMES: usize = 65_536;

/// How the host will run the processor.
///
/// The main bus only. Open: a sidechain or any other extra bus is not
/// designed yet, and needs its own shape before a plugin that has one.
///
/// # Examples
///
/// ```
/// use hapto_host::ProcessSetup;
/// use hapto_host::layout::Width;
///
/// // A host's activation for a stereo effect: a processor sizes everything
/// // it needs from this, once, and the audio thread never allocates again.
/// let mut setup = ProcessSetup::new(44_100.0, 1_024, Width::STEREO);
/// setup.max_events = 256;
/// let scratch = vec![0.0_f32; setup.max_frames * setup.layout.outputs];
/// assert_eq!(scratch.len(), 2_048);
/// ```
// Made with `new` outside this crate, so the buses beyond the main one can
// join it without breaking a caller.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct ProcessSetup {
    /// Samples per second.
    pub sample_rate: f64,
    /// The most frames a block will hold, at most [`MAX_FRAMES`].
    pub max_frames: usize,
    /// The main bus's channels in and out: the width the host chose among
    /// those the processor's [layout](crate::Processor::Layout) runs at.
    pub layout: Width,
    /// The most MIDI packets one block may carry in; more are dropped and
    /// counted. A block's decoded messages have room for twice as many,
    /// since one note-off may end several notes on its key.
    pub max_events: usize,
    /// The most MIDI events one block may send; more are dropped and
    /// counted. Zero for a plugin without
    /// [`midi_output`](crate::PluginInfo::midi_output).
    pub max_out_events: usize,
}

impl ProcessSetup {
    /// A setup at `sample_rate` for blocks of up to `max_frames`, with the
    /// main bus at `layout` and no MIDI in or out. A processor that takes or
    /// sends MIDI sets [`max_events`](Self::max_events) or
    /// [`max_out_events`](Self::max_out_events) after.
    pub const fn new(sample_rate: f64, max_frames: usize, layout: Width) -> Self {
        ProcessSetup {
            sample_rate,
            max_frames,
            layout,
            max_events: 0,
            max_out_events: 0,
        }
    }
}

/// One block, as [`Processor::process`](crate::Processor::process) gets it.
///
/// Everything here is lent by the exporter for one call. A sidechain or
/// transport joins this struct when it is designed, so adding one does not
/// change the trait.
///
/// # Examples
///
/// ```
/// use hapto_host::layout::AnyLayout;
/// use hapto_host::{Block, Processor, Status};
///
/// /// Halves the level, in place, in every channel, whatever the width.
/// struct Half;
///
/// impl Processor for Half {
///     type Layout = AnyLayout;
///
///     fn process(&mut self, mut block: Block<'_, AnyLayout>) -> Status {
///         for channel in block.audio.channels_mut() {
///             for sample in channel {
///                 *sample *= 0.5;
///             }
///         }
///         Status::Continue
///     }
/// }
/// # use hapto_host::{Audio, EventList, Midi, MidiOut};
/// # let mut out = EventList::with_capacity(0);
/// # let mut left = [1.0_f32; 4];
/// # let mut channels = [&mut left[..]];
/// # let block = Block::new(Audio::new(&mut channels, 4), Midi::default(), MidiOut::new(&mut out));
/// # Half.process(block);
/// # assert_eq!(left, [0.5; 4]);
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct Block<'a, L: Layout> {
    /// The main bus's channels over the block's frames, at most
    /// [`ProcessSetup::max_frames`], processed in place: as many as the
    /// larger of the setup's [`layout`](ProcessSetup::layout)'s inputs and
    /// outputs.
    ///
    /// On entry the first `inputs` carry the input, and any further ones are
    /// zeroed. On return the first `outputs` carry the output.
    pub audio: Audio<'a, L>,
    /// The MIDI that arrived, decoded once by the engine: each
    /// [`Message`](crate::Message) at its frame, in the order it came, with
    /// the packets it came in. Read it as often as needed, as a `MidiBuffer`
    /// is read.
    pub midi: Midi<'a>,
    /// The MIDI to send, in time order: messages, or packets as they are.
    /// MIDI only: parameter changes reach the host through the exporter.
    ///
    /// Empty on entry, but for a host's block longer than
    /// [`ProcessSetup::max_frames`], which plays as several blocks. Each
    /// later one finds what the earlier ones sent, already at frames from
    /// the host's block's start. The exporter hands it to the host once the
    /// block has played, for a plugin that declares
    /// [`PluginInfo::midi_output`](crate::PluginInfo::midi_output).
    pub midi_out: MidiOut<'a>,
}

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
/// use hapto_host::Status;
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
    /// Keep processing for the plugin's [`tail`](crate::Plugin::tail) once
    /// the input is silent.
    Tail,
    /// Nothing to do until input or events arrive again.
    Sleep,
}

impl<'a, L: Layout> Block<'a, L> {
    /// Creates a block over `audio`, with `midi` in and `midi_out` to send
    /// into, for an exporter or a test that drives a processor.
    /// [`Midi::default`] is a block with no MIDI in.
    pub fn new(audio: Audio<'a, L>, midi: Midi<'a>, midi_out: MidiOut<'a>) -> Self {
        Block {
            audio,
            midi,
            midi_out,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_processor_sends_midi_through_its_block() {
        use crate::Processor;
        use crate::layout::Mono;

        struct Echo;
        impl Processor for Echo {
            type Layout = Mono;

            fn process(&mut self, mut block: Block<'_, Mono>) -> Status {
                block.midi_out.extend(block.midi.packets());
                block.audio.clear();
                Status::Sleep
            }
        }

        let mut events = EventList::with_capacity(2);
        // The first packet of a MIDI-CI Discovery inquiry, as SysEx7.
        events.push(Event::new(3, [0x3016_7E7F, 0x0D70_0201, 0, 0]));
        let mut list = hapto_midi::MidiList::new();
        list.read(&events, 4);
        let mut out = EventList::with_capacity(1);
        let mut left = [1.0_f32; 4];
        let mut channels = [&mut left[..]];
        let block = Block::new(
            Audio::new(&mut channels, 4),
            list.midi(),
            MidiOut::new(&mut out),
        );
        let status = Echo.process(block);
        assert_eq!(status, Status::Sleep);
        assert_eq!(out.as_slice(), events.as_slice());
        assert_eq!(left, [0.0; 4]);
    }
}
