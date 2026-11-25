//! A plugin played without a host for its tests, at any block sizes, with
//! MIDI in each form that a host sends, and rendered into a [`Sound`].

use std::ops::Range;

use hapto_midi::Key;
use hapto_ump::{Attribute, ByteStream, ChannelMessage, ChannelVoice, Origin, scale};

use crate::layout::Width;
use crate::standalone::{Standalone, StandaloneAudio, StandaloneBlock};
use crate::{Event, Plugin, widths};

/// A [`Play`]'s block size, unless [`Play::with_blocks`] sets the sizes.
const BLOCK: usize = 512;

/// The level below which a [`Sound`] is silent: -80 dB.
const SILENT: f32 = 1e-4;

/// A plugin played without a host, so that a test can send it MIDI at
/// frames from the play's start and render it into a [`Sound`].
///
/// It plays the plugin through the engine that every exporter drives, so
/// MIDI 1.0 is upgraded to MIDI 2.0 as a host's is, and the notes it names
/// reach the plugin as VST3's do. Blocks are 512 frames unless
/// [`with_blocks`](Self::with_blocks) sets other sizes, and a plugin's sound
/// should not depend on them.
///
/// Thread: the one that creates it, which is both the plugin's main thread
/// and its audio thread.
///
/// # Panics
///
/// [`render`](Self::render) panics if the plugin panicked in a block, which
/// would have been silent in a host.
///
/// # Examples
///
/// ```
#[doc = include_str!("../doc/synth.rs")]
/// use hapto_host::play::Play;
///
/// let mut play = Play::<Synth>::new(48_000.0);
/// // A4 at full velocity for a tenth of a second, as MIDI 1.0's bytes.
/// play.midi1(0, &[0x90, 69, 127]);
/// play.midi1(4_800, &[0x80, 69, 0]);
/// let sound = play.render(9_600);
/// assert!(sound.peak(0..4_800) > 0.9);
/// assert!(sound.is_silent(4_800..9_600));
/// ```
pub struct Play<P: Plugin> {
    // Dropped before `app`, which `Standalone` requires.
    audio: StandaloneAudio<P>,
    app: Standalone<P>,
    sample_rate: f64,
    layout: Width,
    blocks: Vec<usize>,
    /// The index in `blocks` of the next block's size.
    next_block: usize,
    /// The packets not yet played, each at its frame from the play's start,
    /// in the order sent.
    queue: Vec<(usize, Event)>,
    /// The number of frames played.
    played: usize,
    bytes: ByteStream,
    /// The host's name for the next note.
    next_note: u32,
}

/// A note that [`Play::note_on`] started and named as a host does, so that a
/// test can end, tune or press it by its name.
///
/// # Examples
///
/// ```
#[doc = include_str!("../doc/synth.rs")]
/// use hapto_host::play::Play;
/// use hapto_midi::Key;
///
/// let mut play = Play::<Synth>::new(48_000.0);
/// let note = play.note_on(0, Key::A4, 1.0);
/// // A4 tuned a semitone up, then pressed halfway, by its name.
/// play.tuning(240, note, 1.0);
/// play.pressure(240, note, 0.5);
/// play.note_off(480, note);
/// assert_eq!(note.key, Key::A4);
/// assert!(play.render(960).is_silent(480..960));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostNote {
    /// The host's name for it.
    pub id: u32,
    /// Its key.
    pub key: Key,
}

impl<P: Plugin> Play<P> {
    /// Returns a play of the plugin, activated at `sample_rate` in the first
    /// layout that it lists, with no MIDI sent yet.
    ///
    /// # Panics
    ///
    /// When the plugin lists no layout or refuses to activate, because the
    /// test can't play it then.
    pub fn new(sample_rate: f64) -> Self {
        let Some(&layout) = widths::<P>().first() else {
            panic!("{} lists no layout to play in", P::INFO.name);
        };
        let app = Standalone::new();
        let audio = app.audio();
        let mut play = Play {
            audio,
            app,
            sample_rate,
            layout,
            blocks: vec![BLOCK],
            next_block: 0,
            queue: Vec::new(),
            played: 0,
            bytes: ByteStream::new(0),
            next_note: 0,
        };
        play.activate();
        play
    }

    /// Returns the play with blocks of `sizes`, taken in turn and repeated,
    /// and the plugin activated again for the largest, to check that its
    /// sound doesn't depend on them.
    ///
    /// # Panics
    ///
    /// When `sizes` is empty or holds a 0, or the plugin refuses to activate
    /// for them.
    ///
    /// # Examples
    ///
    /// ```
    #[doc = include_str!("../doc/synth.rs")]
    /// use hapto_host::play::Play;
    ///
    /// let render = |blocks: &[usize]| {
    ///     let mut play = Play::<Synth>::new(48_000.0).with_blocks(blocks);
    ///     play.midi1(100, &[0x90, 60, 100]);
    ///     play.midi1(2_000, &[0x80, 60, 0]);
    ///     play.render(4_000)
    /// };
    /// assert_eq!(render(&[1, 999, 13, 64]), render(&[4_000]));
    /// ```
    #[must_use]
    pub fn with_blocks(mut self, sizes: &[usize]) -> Self {
        assert!(
            !sizes.is_empty() && !sizes.contains(&0),
            "a block is at least one frame, and there is at least one"
        );
        self.blocks = sizes.to_vec();
        self.activate();
        self
    }

    /// Returns the play in `layout`, one of those the plugin lists, with the
    /// plugin activated again for it.
    ///
    /// # Panics
    ///
    /// When the plugin refuses to activate in `layout`.
    #[must_use]
    pub fn with_layout(mut self, layout: Width) -> Self {
        self.layout = layout;
        self.activate();
        self
    }

    fn activate(&mut self) {
        let most = self.blocks.iter().copied().max().unwrap_or(BLOCK);
        self.app.deactivate();
        if let Err(refused) = self.app.activate(self.sample_rate, most, self.layout) {
            panic!("{} refused to activate: {refused}", P::INFO.name);
        }
    }

    /// Returns the app that holds the plugin, with its parameters, saved
    /// state and tick.
    pub fn app(&self) -> &Standalone<P> {
        &self.app
    }

    /// Sends MIDI 1.0 `bytes` at `frame` on the first group, read as a
    /// device's byte stream, including running status and SysEx.
    ///
    /// # Panics
    ///
    /// When `frame` has already played.
    pub fn midi1(&mut self, frame: usize, bytes: &[u8]) {
        let mut packets = Vec::new();
        self.bytes.read(bytes, |words| packets.push(words));
        for words in packets {
            self.packet(frame, Event::new(0, words));
        }
    }

    /// Sends a MIDI 2.0 channel voice message at `frame`, on the first
    /// group and channel.
    ///
    /// # Panics
    ///
    /// When `frame` has already played.
    ///
    /// # Examples
    ///
    /// ```
    #[doc = include_str!("../doc/synth.rs")]
    /// use hapto_host::play::Play;
    /// use hapto_ump::{Attribute, ChannelMessage};
    ///
    /// let mut play = Play::<Synth>::new(48_000.0);
    /// let (note, velocity, attribute) = (69, u16::MAX, Attribute::NONE);
    /// play.midi2(0, ChannelMessage::NoteOn { note, velocity, attribute });
    /// assert!(play.render(480).peak(0..480) > 0.9);
    /// ```
    pub fn midi2(&mut self, frame: usize, message: ChannelMessage) {
        self.voice(frame, message, Origin::Midi2);
    }

    /// Sends `event`, with any group, channel, message and origin, at
    /// `frame` rather than at its offset.
    ///
    /// # Panics
    ///
    /// When `frame` has already played.
    pub fn packet(&mut self, frame: usize, event: Event) {
        assert!(
            frame >= self.played,
            "frame {frame} has played: the play is at frame {}",
            self.played
        );
        self.queue.push((frame, event));
    }

    /// Starts a note at `frame` on the first group and channel, at
    /// `velocity` from 0 to 1, and names it as a host such as VST3 does.
    ///
    /// # Panics
    ///
    /// When `frame` has already played.
    ///
    /// # Examples
    ///
    /// ```
    #[doc = include_str!("../doc/synth.rs")]
    /// use hapto_host::play::Play;
    /// use hapto_midi::Key;
    ///
    /// // Two notes on one key, which their names keep apart.
    /// let mut play = Play::<Synth>::new(48_000.0);
    /// let first = play.note_on(0, Key::A4, 1.0);
    /// play.note_on(480, Key::A4, 1.0);
    /// play.note_off(960, first);
    /// let sound = play.render(1_440);
    /// assert!(sound.peak(960..1_440) > 0.9, "the second sounds on");
    /// ```
    pub fn note_on(&mut self, frame: usize, key: Key, velocity: f32) -> HostNote {
        let note = HostNote {
            id: self.next_note,
            key,
        };
        self.next_note = self.next_note.wrapping_add(1);
        let message = ChannelMessage::NoteOn {
            note: key.wire(),
            velocity: scale::unit_to_16(f64::from(velocity)),
            attribute: Attribute::NONE,
        };
        self.voice(frame, message, Origin::Note { id: note.id });
        note
    }

    /// Ends `note` at `frame`.
    ///
    /// # Panics
    ///
    /// When `frame` has already played.
    pub fn note_off(&mut self, frame: usize, note: HostNote) {
        let message = ChannelMessage::NoteOff {
            note: note.key.wire(),
            velocity: 0,
            attribute: Attribute::NONE,
        };
        self.voice(frame, message, Origin::Note { id: note.id });
    }

    /// Tunes `note` `semitones` from its key at `frame`, as VST3's tuning
    /// expression does.
    ///
    /// # Panics
    ///
    /// When `frame` has already played.
    pub fn tuning(&mut self, frame: usize, note: HostNote, semitones: f32) {
        let pitch = f64::from(note.key.wire()) + f64::from(semitones);
        let message = ChannelMessage::RegisteredPerNoteController {
            note: note.key.wire(),
            index: 3,
            value: scale::pitch_7_25(pitch),
        };
        self.voice(frame, message, Origin::Note { id: note.id });
    }

    /// Presses `note` to `pressure`, from 0 to 1, at `frame`, as VST3's
    /// poly pressure does.
    ///
    /// # Panics
    ///
    /// When `frame` has already played.
    pub fn pressure(&mut self, frame: usize, note: HostNote, pressure: f32) {
        let message = ChannelMessage::PolyPressure {
            note: note.key.wire(),
            value: scale::unit_to_32(f64::from(pressure)),
        };
        self.voice(frame, message, Origin::Note { id: note.id });
    }

    fn voice(&mut self, frame: usize, message: ChannelMessage, origin: Origin) {
        let voice = ChannelVoice {
            group: 0,
            channel: 0,
            message,
        };
        self.packet(
            frame,
            Event {
                origin,
                ..voice.event(0)
            },
        );
    }

    /// Plays the next `frames` frames in blocks, with silent inputs and the
    /// MIDI sent for them, and returns the output.
    ///
    /// # Panics
    ///
    /// When the plugin panicked in a block.
    pub fn render(&mut self, frames: usize) -> Sound {
        let mut channels = vec![vec![0.0_f32; frames]; self.layout.outputs];
        let largest = self.blocks.iter().copied().max().unwrap_or(BLOCK);
        let silence = vec![0.0_f32; largest];
        let inputs: Vec<&[f32]> = (0..self.layout.inputs).map(|_| &silence[..]).collect();
        let faults = self.app.faults();
        let mut sent = Vec::new();
        let mut at = 0;
        while at < frames {
            let size = self.blocks.get(self.next_block).copied().unwrap_or(BLOCK);
            self.next_block = (self.next_block + 1) % self.blocks.len();
            let length = size.min(frames - at);
            let (start, end) = (self.played + at, self.played + at + length);
            let midi = self.take(start, end);
            let mut outputs: Vec<&mut [f32]> = channels
                .iter_mut()
                .map(|channel| &mut channel[at..at + length])
                .collect();
            let inputs: Vec<&[f32]> = inputs.iter().map(|input| &input[..length]).collect();
            let block = StandaloneBlock::new(&inputs, &mut outputs).with_midi(length, &midi);
            self.audio.process(block, |out| {
                sent.extend(out.iter().map(|event| Event {
                    offset: event.offset + u32::try_from(start).unwrap_or(u32::MAX),
                    ..*event
                }));
            });
            at += length;
        }
        self.played += frames;
        assert_eq!(
            self.app.faults(),
            faults,
            "{} panicked or was refused while it played",
            P::INFO.name
        );
        Sound { channels, sent }
    }

    /// Removes and returns the packets for frames `start` to `end`, each at
    /// its frame of the block, in the order sent.
    fn take(&mut self, start: usize, end: usize) -> Vec<Event> {
        let mut block = Vec::new();
        self.queue.retain(|&(frame, event)| {
            let due = frame < end;
            if due {
                let offset = u32::try_from(frame.saturating_sub(start)).unwrap_or(u32::MAX);
                block.push(Event { offset, ..event });
            }
            !due
        });
        block
    }
}

impl<P: Plugin> std::fmt::Debug for Play<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Play")
            .field("plugin", &P::INFO.name)
            .field("played", &self.played)
            .field("blocks", &self.blocks)
            .finish_non_exhaustive()
    }
}

/// Each output channel's samples from a [`Play`]'s render, and the MIDI that
/// the plugin sent, each packet at its frame of the render.
///
/// Two sounds are equal when every sample and packet is, bit for bit.
/// `Debug` prints a summary, so a failed comparison prints two lines, and
/// [`difference`](Self::difference) returns where two first differ.
///
/// # Examples
///
/// ```
#[doc = include_str!("../doc/synth.rs")]
/// use hapto_host::play::Play;
///
/// let mut play = Play::<Synth>::new(48_000.0);
/// play.midi1(240, &[0x90, 69, 64]);
/// let sound = play.render(480);
/// assert_eq!(sound.channels().len(), 2);
/// assert!(sound.is_silent(0..240));
/// assert!(sound.peak(240..480) > 0.4);
/// ```
#[derive(Clone, PartialEq)]
pub struct Sound {
    channels: Vec<Vec<f32>>,
    sent: Vec<Event>,
}

/// Where two [`Sound`]s first differ, and each one's sample there.
///
/// # Examples
///
/// ```
#[doc = include_str!("../doc/synth.rs")]
/// use hapto_host::play::Play;
///
/// let mut play = Play::<Synth>::new(48_000.0);
/// let silence = play.render(480);
/// play.midi1(480, &[0x90, 69, 127]);
/// let note = play.render(480);
/// let difference = note.difference(&silence).expect("the note");
/// assert_eq!((difference.frame, difference.other), (1, Some(0.0)));
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Difference {
    /// The channel, from 0.
    pub channel: usize,
    /// The frame of the render.
    pub frame: usize,
    /// The sample in this sound.
    pub this: f32,
    /// The sample in `other`, or `None` past its end or its channels.
    pub other: Option<f32>,
}

impl Sound {
    /// Returns each output channel's samples, in order.
    pub fn channels(&self) -> &[Vec<f32>] {
        &self.channels
    }

    /// Returns the MIDI that the plugin sent, each packet at its frame of the
    /// render.
    pub fn sent(&self) -> &[Event] {
        &self.sent
    }

    /// Returns the largest absolute sample in any channel over `frames`,
    /// clamped to the render's length.
    pub fn peak(&self, frames: Range<usize>) -> f32 {
        self.channels
            .iter()
            .flat_map(|channel| {
                let end = frames.end.min(channel.len());
                channel.get(frames.start.min(end)..end).unwrap_or_default()
            })
            .fold(0.0, |peak, sample| peak.max(sample.abs()))
    }

    /// Returns whether every sample over `frames` is below -80 dB.
    pub fn is_silent(&self, frames: Range<usize>) -> bool {
        self.peak(frames) < SILENT
    }

    /// Returns the first sample, by channel then frame, at which this sound
    /// and `other` differ, or `None` if every sample is the same. The MIDI
    /// they sent isn't compared.
    ///
    /// # Examples
    ///
    /// ```
    #[doc = include_str!("../doc/synth.rs")]
    /// use hapto_host::play::Play;
    ///
    /// let render = |frame| {
    ///     let mut play = Play::<Synth>::new(48_000.0);
    ///     play.midi1(frame, &[0x90, 69, 127]);
    ///     play.render(480)
    /// };
    /// let difference = render(100).difference(&render(101));
    /// let difference = difference.expect("a frame apart");
    /// assert_eq!((difference.channel, difference.frame), (0, 101));
    /// ```
    pub fn difference(&self, other: &Sound) -> Option<Difference> {
        self.channels
            .iter()
            .enumerate()
            .find_map(|(channel, samples)| {
                let theirs = other.channels.get(channel);
                samples.iter().enumerate().find_map(|(frame, &this)| {
                    let other = theirs.and_then(|theirs| theirs.get(frame)).copied();
                    (other != Some(this)).then_some(Difference {
                        channel,
                        frame,
                        this,
                        other,
                    })
                })
            })
    }
}

impl std::fmt::Debug for Sound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A fingerprint of every sample's bits, so two sounds that differ
        // print differently.
        let fingerprint = self
            .channels
            .iter()
            .flatten()
            .fold(0xCBF2_9CE4_8422_2325_u64, |hash, sample| {
                (hash ^ u64::from(sample.to_bits())).wrapping_mul(0x0100_0000_01B3)
            });
        let frames = self.channels.first().map_or(0, Vec::len);
        f.debug_struct("Sound")
            .field("channels", &self.channels.len())
            .field("frames", &frames)
            .field("peak", &self.peak(0..frames))
            .field("fingerprint", &format_args!("{fingerprint:016x}"))
            .field("sent", &self.sent.len())
            .finish()
    }
}
