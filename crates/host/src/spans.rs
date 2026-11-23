//! A block cut into spans: the stretches between its MIDI messages, in
//! which nothing arrives.

// Audio-path code: a panic here ends a host, so indexing, unwrap, expect,
// panic and unchecked arithmetic are refused, and a site that is provably
// safe says why in an `#[expect]`.
#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects
    )
)]

use crate::layout::Layout;
use crate::{Audio, Block, Midi};

/// Part of a block in which nothing arrives but at its first frame: its
/// audio, and the messages that arrive as it starts.
///
/// A span has a block's shape, so it goes to any function written for a
/// block's audio.
///
/// # Examples
///
/// ```
/// use hapto_host::layout::Mono;
/// use hapto_host::{AudioBuf, Midi, Spans};
///
/// let mut buffer = AudioBuf::<Mono>::new(6);
/// for mut span in Spans::new(buffer.audio(), Midi::default()).at_most(4) {
///     // The block's frame it starts at, and its own frames.
///     let level = span.start as f32;
///     span.audio.add_to_all(std::iter::repeat(level));
/// }
/// assert_eq!(buffer.channels()[0], [0.0, 0.0, 0.0, 0.0, 4.0, 4.0]);
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct Span<'a, L: Layout> {
    /// The block's frame the span starts at: for sending MIDI at the
    /// block's frames from inside it.
    pub start: usize,
    /// The span's frames, in every channel.
    pub audio: Audio<'a, L>,
    /// What arrives at its first frame, in the order it came: every message
    /// is at frame 0, so [`Midi::messages`] reads them.
    pub midi: Midi<'a>,
}

/// A block's spans, in time order, disjoint, covering it once: a new one at
/// each frame a message arrives at, so a plugin acts on each message, then
/// renders until the next. None is empty. [`Block::spans`] makes them.
///
/// A plugin that never asks for spans keeps its block whole, so a convolver
/// or an FFT is never cut short.
///
/// Open: a parameter marked sample-accurate, to split at its automation
/// too, is to come.
///
/// # Examples
///
/// ```
/// use hapto_host::layout::Mono;
/// use hapto_host::midi::{Message, controller};
/// use hapto_host::{Block, Processor, Status};
///
/// /// A gate the sustain pedal opens: each span plays at the pedal's state
/// /// as it starts.
/// struct Pedal {
///     down: bool,
/// }
///
/// impl Processor for Pedal {
///     type Layout = Mono;
///
///     fn process(&mut self, block: Block<'_, Mono>) -> Status {
///         for mut span in block.spans() {
///             for message in span.midi.messages() {
///                 if let Some(pedal) = controller::SUSTAIN.read(&message) {
///                     self.down = pedal.is_down();
///                 }
///             }
///             if !self.down {
///                 span.audio.clear();
///             }
///         }
///         Status::Continue
///     }
/// }
/// # use hapto_host::{Audio, EventList, MidiOut};
/// # use hapto_host::midi::MidiList;
/// # let mut packets = EventList::with_capacity(1);
/// # packets.push(hapto_host::Event::new(4, [0x40B0_4000, u32::MAX, 0, 0]));
/// # let mut list = MidiList::new();
/// # list.read(&packets, 8);
/// # let (mut samples, mut out) = ([1.0_f32; 8], EventList::with_capacity(0));
/// # let mut channels = [&mut samples[..]];
/// # let block = Block::new(Audio::new(&mut channels, 8), list.midi(), MidiOut::new(&mut out));
/// # Pedal { down: false }.process(block);
/// // Silent until the pedal goes down at frame 4.
/// # assert_eq!(samples, [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]);
/// ```
#[derive(Debug)]
pub struct Spans<'a, L: Layout> {
    /// The frames not yet handed out; `None` once all are.
    rest: Option<Audio<'a, L>>,
    midi: Midi<'a>,
    /// The block's frame `rest` starts at.
    at: usize,
    /// The most frames a span holds.
    most: usize,
}

impl<'a, L: Layout> Spans<'a, L> {
    /// The spans of `audio` with `midi`, a block's: for a plugin that sends
    /// MIDI too, which keeps the block's `midi_out` and makes its spans from
    /// the rest.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_host::layout::Mono;
    /// use hapto_host::{Block, Processor, Spans, Status};
    ///
    /// /// Sends a clock tick at the start of each span.
    /// struct Ticks;
    ///
    /// impl Processor for Ticks {
    ///     type Layout = Mono;
    ///
    ///     fn process(&mut self, block: Block<'_, Mono>) -> Status {
    ///         let Block { audio, midi, mut midi_out, .. } = block;
    ///         for span in Spans::new(audio, midi) {
    ///             midi_out.send_packet(hapto_host::Event::new(span.start as u32, [0x10F8_0000, 0, 0, 0]));
    ///         }
    ///         Status::Continue
    ///     }
    /// }
    /// # use hapto_host::{Audio, EventList, Midi, MidiOut};
    /// # let (mut samples, mut out) = ([0.0_f32; 8], EventList::with_capacity(4));
    /// # let mut channels = [&mut samples[..]];
    /// # let block = Block::new(Audio::new(&mut channels, 8), Midi::default(), MidiOut::new(&mut out));
    /// # Ticks.process(block);
    /// # assert_eq!(out.as_slice().len(), 1, "a block with no MIDI is one span");
    /// ```
    pub fn new(audio: Audio<'a, L>, midi: Midi<'a>) -> Self {
        Spans {
            rest: (audio.frames() > 0).then_some(audio),
            midi,
            at: 0,
            most: usize::MAX,
        }
    }

    /// Also splits wherever a span would run past `frames`, at least 1: for
    /// an effect whose modulation must move at least that often.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_host::{AudioBuf, Midi, Spans};
    /// use hapto_host::layout::Mono;
    ///
    /// let mut buffer = AudioBuf::<Mono>::new(10);
    /// let lengths: Vec<usize> = Spans::new(buffer.audio(), Midi::default())
    ///     .at_most(4)
    ///     .map(|span| span.audio.frames())
    ///     .collect();
    /// assert_eq!(lengths, [4, 4, 2]);
    /// ```
    pub fn at_most(mut self, frames: usize) -> Self {
        self.most = frames.max(1);
        self
    }
}

impl<'a, L: Layout> Iterator for Spans<'a, L> {
    type Item = Span<'a, L>;

    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    fn next(&mut self) -> Option<Span<'a, L>> {
        let rest = self.rest.take()?;
        let left = rest.frames();
        // The next message after this span's first frame ends it.
        let next = self
            .midi
            .between(self.at.saturating_add(1), usize::MAX)
            .first_frame()
            .map_or(left, |frame| frame.saturating_add(1));
        let length = next.min(self.most).min(left).max(1);
        let (audio, later) = rest.split_at(length);
        let start = self.at;
        let midi = self.midi.between(start, start.saturating_add(1));
        self.at = start.saturating_add(length);
        self.rest = (later.frames() > 0).then_some(later);
        Some(Span { start, audio, midi })
    }
}

impl<'a, L: Layout> Block<'a, L> {
    /// Splits the block at each frame a MIDI message arrives at: see
    /// [`Spans`]. The block's [`midi_out`](Self::midi_out) goes with it; a
    /// plugin that sends too makes its spans with [`Spans::new`].
    pub fn spans(self) -> Spans<'a, L> {
        Spans::new(self.audio, self.midi)
    }
}
