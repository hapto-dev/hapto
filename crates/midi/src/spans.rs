//! Audio cut into spans, the stretches between its MIDI messages.

use hapto_audio::Audio;
use hapto_audio::layout::Layout;

use crate::Midi;

/// Part of a block in which messages arrive only at its first frame.
///
/// A span's fields match a block's, so any function written for a block's
/// audio also takes a span's.
///
/// # Examples
///
/// ```
/// use hapto_audio::AudioBuf;
/// use hapto_audio::layout::Mono;
/// use hapto_midi::{Midi, Spans};
///
/// let mut buffer = AudioBuf::<Mono>::new(6);
/// for mut span in Spans::new(buffer.audio(), Midi::default()).at_most(4) {
///     // The span's start, in the block's frames, and its own audio.
///     let level = span.start as f32;
///     span.audio.add_to_all(std::iter::repeat(level));
/// }
/// assert_eq!(buffer.channels()[0], [0.0, 0.0, 0.0, 0.0, 4.0, 4.0]);
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct Span<'a, L: Layout> {
    /// The block's frame that the span starts at, for sending MIDI at the
    /// block's frames from inside it.
    pub start: usize,
    /// The span's frames, in every channel.
    pub audio: Audio<'a, L>,
    /// The messages at the first frame, in the order they came. Each is at
    /// frame 0, so [`Midi::messages`] reads them.
    pub midi: Midi<'a>,
}

/// A block's spans in time order, which cover each frame once.
///
/// A new span starts at each frame where a message arrives, so a plugin acts
/// on each message and then renders until the next. None is empty, and a
/// block's `spans` method makes them.
///
/// A plugin that never cuts its block into spans keeps it whole, so a
/// convolver or an FFT is never cut short.
///
/// Open: marking a parameter sample-accurate, so spans also split at its
/// automation, is to come.
///
/// # Examples
///
/// ```
/// use hapto_audio::AudioBuf;
/// use hapto_audio::layout::Mono;
/// use hapto_midi::controller::SUSTAIN;
/// use hapto_midi::{MidiList, Spans};
/// use hapto_ump::{Event, EventList};
///
/// // The sustain pedal down at frame 4, in a block of 8.
/// let mut packets = EventList::with_capacity(1);
/// packets.push(Event::new(4, [0x40B0_4000, u32::MAX, 0, 0]));
/// let mut list = MidiList::new();
/// list.read(&packets, 8);
/// let mut buffer = AudioBuf::<Mono>::new(8);
/// buffer.audio().add_to_all(std::iter::repeat(1.0));
///
/// // A gate that the pedal opens, so each span plays with the pedal's
/// // state at its start.
/// let mut down = false;
/// for mut span in Spans::new(buffer.audio(), list.midi()) {
///     for message in span.midi.messages() {
///         if let Some(pedal) = SUSTAIN.read(&message) {
///             down = pedal.is_down();
///         }
///     }
///     if !down {
///         span.audio.clear();
///     }
/// }
/// assert_eq!(buffer.channels()[0], [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]);
/// ```
#[derive(Debug)]
pub struct Spans<'a, L: Layout> {
    /// The frames not yet handed out, or `None` once all are.
    rest: Option<Audio<'a, L>>,
    midi: Midi<'a>,
    /// The block's frame that `rest` starts at.
    at: usize,
    /// The most frames a span holds.
    most: usize,
}

impl<'a, L: Layout> Spans<'a, L> {
    /// Returns the spans of `audio` with `midi`, a block's or part of one.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_audio::AudioBuf;
    /// use hapto_audio::layout::Mono;
    /// use hapto_midi::{MidiList, Spans};
    /// use hapto_ump::{Event, EventList};
    ///
    /// // Messages at frames 2 and 5 of a block of 8.
    /// let mut packets = EventList::with_capacity(2);
    /// packets.push(Event::new(2, [0x40B0_0700, u32::MAX, 0, 0]));
    /// packets.push(Event::new(5, [0x40B0_0700, 0, 0, 0]));
    /// let mut list = MidiList::new();
    /// list.read(&packets, 8);
    /// let mut buffer = AudioBuf::<Mono>::new(8);
    /// let lengths: Vec<usize> = Spans::new(buffer.audio(), list.midi())
    ///     .map(|span| span.audio.frames())
    ///     .collect();
    /// assert_eq!(lengths, [2, 3, 3]);
    /// ```
    pub fn new(audio: Audio<'a, L>, midi: Midi<'a>) -> Self {
        Spans {
            rest: (audio.frames() > 0).then_some(audio),
            midi,
            at: 0,
            most: usize::MAX,
        }
    }

    /// Also splits wherever a span would run past `frames` (at least 1), for
    /// an effect whose modulation must move at least that often.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_audio::AudioBuf;
    /// use hapto_audio::layout::Mono;
    /// use hapto_midi::{Midi, Spans};
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
        // Every bound is at least 1 (`at_most` keeps `most` above 0 and
        // `rest` is never empty), so each span moves `at` forward.
        let length = next.min(self.most).min(left);
        let (audio, later) = rest.split_at(length);
        let start = self.at;
        let midi = self.midi.between(start, start.saturating_add(1));
        self.at = start.saturating_add(length);
        self.rest = (later.frames() > 0).then_some(later);
        Some(Span { start, audio, midi })
    }
}
