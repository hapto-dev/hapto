//! A block cut into spans, or played by voices: `Spans` and `Poly` are
//! hapto-midi's, since each is only audio and MIDI, and a block makes them.

use hapto_midi::ToPlay;

use crate::layout::Layout;
use crate::{Block, MidiOut, Spans};

impl<'a, L: Layout> Block<'a, L> {
    /// Splits the block at each frame a MIDI message arrives at: see
    /// [`Spans`]. The block's [`midi_out`](Self::midi_out) goes with it; a
    /// plugin that sends too takes both with
    /// [`spans_and_out`](Self::spans_and_out).
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_host::layout::Mono;
    /// use hapto_host::midi::controller;
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
    pub fn spans(self) -> Spans<'a, L> {
        Spans::new(self.audio, self.midi)
    }

    /// Splits the block as [`spans`](Self::spans) does and also returns its
    /// [`midi_out`](Self::midi_out), for a plugin that sends MIDI as it plays
    /// each span.
    ///
    /// # Examples
    ///
    /// ```
    /// use hapto_host::layout::Mono;
    /// use hapto_host::{Block, Event, Processor, Status};
    ///
    /// /// Sends a MIDI clock tick at the start of each span.
    /// struct Ticks;
    ///
    /// impl Processor for Ticks {
    ///     type Layout = Mono;
    ///
    ///     fn process(&mut self, block: Block<'_, Mono>) -> Status {
    ///         let (spans, mut midi_out) = block.spans_and_out();
    ///         for span in spans {
    ///             midi_out.send_packet(Event::new(span.start as u32, [0x10F8_0000, 0, 0, 0]));
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
    pub fn spans_and_out(self) -> (Spans<'a, L>, MidiOut<'a>) {
        (Spans::new(self.audio, self.midi), self.midi_out)
    }
}

/// A block's audio and MIDI, for [`Poly::play`](hapto_midi::Poly::play):
/// `self.voices.play(block)`. What the block sends is dropped; a plugin that
/// sends hands the two over itself, `(block.audio, block.midi)`, and keeps
/// `block.midi_out`.
impl<'a, L: Layout> From<Block<'a, L>> for ToPlay<'a, L> {
    fn from(block: Block<'a, L>) -> Self {
        ToPlay::new(block.audio, block.midi)
    }
}
