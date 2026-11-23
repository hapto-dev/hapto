//! The block: what the audio thread works with, and one call of it.

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

#[cfg(loom)]
use loom::sync::atomic::AtomicBool;
use std::num::NonZeroUsize;
use std::panic::{self, AssertUnwindSafe};
#[cfg(not(loom))]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use hapto_learn::LearnAudio;
use hapto_midi::{Decoder, MidiList, MidiOut};
use hapto_params::audio_thread;
use hapto_ump::Upgrade;

use super::{Engine, HostLink, Points, Setup};
use crate::boundary::{AudioSlot, Channels, FlushDenormals};
use crate::faults::Fault;
use crate::layout::Width;
use crate::{Audio, Block, Event, EventList, Plugin, ProcessSetup, Processor, Status};

/// What the audio thread works with while the plugin is active.
#[derive(Debug)]
pub(super) struct Active<R> {
    pub(super) processor: R,
    channels: Channels,
    /// The block's MIDI in as the exporter hands it over, before it is put
    /// in time order.
    arrivals: Arrivals,
    /// The block's MIDI in, at frames from the block's start.
    events: EventList,
    /// MIDI 1.0 the host sends, upgraded as it arrives. It holds an NRPN's
    /// high byte from block to block, so it lives with the processor.
    upgrade: Upgrade,
    /// The block's MIDI in, decoded once: each piece reads its share. It
    /// lives with the processor, as its notes and a SysEx under way span
    /// blocks.
    midi: MidiList,
    /// The block's MIDI out. Every piece pushes to it, and what a piece
    /// pushed moves to the block's frames once the piece returns.
    out_events: EventList,
    /// Learn's audio side, while the plugin's parts have learn on: it runs
    /// over each block's MIDI before the processor does. It comes and goes
    /// with the processor, so it is never dropped on the audio thread.
    pub(super) learn: Option<LearnAudio>,
    /// Set when a block panicked in this processor, which then plays
    /// silence. The flag lives with the processor, so the one an activation
    /// builds starts clean.
    faulted: bool,
}

impl<R> Active<R> {
    /// Asks `plugin` for a processor for `setup` and `layout`, and allocates
    /// everything a block will touch. `events` gives the capacity of each
    /// direction's MIDI list from whether the plugin declares MIDI there.
    pub(super) fn new<P: Plugin<Processor = R>>(
        plugin: &mut P,
        setup: Setup,
        layout: Width,
        events: impl Fn(bool) -> usize,
    ) -> Self {
        let max_events = events(P::INFO.midi_input);
        let max_out_events = events(P::INFO.midi_output);
        let processor = plugin.activate(&ProcessSetup {
            sample_rate: setup.sample_rate,
            max_frames: setup.max_frames.get(),
            layout,
            max_events,
            max_out_events,
        });
        Active::around(
            processor,
            layout,
            setup.max_frames,
            max_events,
            max_out_events,
        )
    }

    /// The last block's MIDI, decoded.
    #[cfg(all(test, feature = "vst3"))]
    pub(super) fn midi(&self) -> hapto_midi::Midi<'_> {
        self.midi.midi()
    }

    fn around(
        processor: R,
        layout: Width,
        max_frames: NonZeroUsize,
        max_events: usize,
        max_out_events: usize,
    ) -> Self {
        Active {
            processor,
            channels: Channels::new(layout.inputs, layout.outputs, max_frames),
            arrivals: Arrivals::with_room(max_events),
            events: EventList::with_capacity(max_events),
            upgrade: Upgrade::new(),
            // A note-off may end several notes, so room for twice the
            // packets.
            midi: MidiList::with_room(
                max_events.saturating_mul(2),
                hapto_midi::BLOCK_SYSEX_BYTES,
                Decoder::new(),
            ),
            out_events: EventList::with_capacity(max_out_events),
            learn: None,
            faulted: false,
        }
    }
}

/// The block's MIDI in, as an exporter fills it: each packet its host
/// sent, at a frame of the block, in any order. The block takes them in
/// time order, those at one frame in the order they came, as VST3 needs:
/// it hands each controller over in a queue of its own, and its notes in a
/// list beside them. A packet of MIDI 1.0 is upgraded to MIDI 2.0 once in
/// order, so nothing above reads MIDI 1.0.
#[cfg_attr(
    not(any(feature = "standalone", feature = "cabi", feature = "vst3", test)),
    expect(dead_code, reason = "no exporter is built")
)]
pub(crate) struct MidiIn<'a> {
    arrivals: &'a mut Arrivals,
}

impl MidiIn<'_> {
    /// Adds the host's next packet. Past the block's room it is dropped and
    /// counted.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    #[cfg_attr(
        not(any(feature = "standalone", feature = "cabi", feature = "vst3", test)),
        expect(dead_code, reason = "no exporter is built")
    )]
    pub(crate) fn push(&mut self, event: Event) {
        self.arrivals.push(event, Rank::Rest);
    }

    /// Adds a packet that goes at its frame as `rank` says, then in the
    /// order it came, for a host that hands controllers over in queues of
    /// their own, as VST3 does: a data entry then finds its address, and a
    /// low byte its high byte, whatever order the queues come in. Past the
    /// block's room it is dropped and counted.
    #[cfg_attr(
        not(feature = "vst3"),
        expect(dead_code, reason = "VST3 alone hands controllers over apart")
    )]
    pub(crate) fn push_ranked(&mut self, event: Event, rank: Rank) {
        self.arrivals.push(event, rank);
    }

    /// Whether the block has room for another packet: an exporter whose
    /// host claims more than it can give stops reading there.
    #[cfg_attr(
        not(feature = "vst3"),
        expect(dead_code, reason = "VST3 alone reads a host's claims")
    )]
    pub(crate) fn has_room(&self) -> bool {
        self.room() > 0
    }

    /// How many more packets the block has room for.
    #[cfg_attr(
        not(feature = "vst3"),
        expect(dead_code, reason = "VST3 alone reads a host's claims")
    )]
    pub(crate) fn room(&self) -> usize {
        self.arrivals
            .packets
            .capacity()
            .saturating_sub(self.arrivals.packets.len())
    }

    /// Counts `count` packets the host sent that the exporter did not read,
    /// for want of room.
    #[cfg_attr(
        not(feature = "vst3"),
        expect(dead_code, reason = "VST3 alone reads a host's claims")
    )]
    pub(crate) fn add_dropped(&mut self, count: u32) {
        self.arrivals.dropped = self.arrivals.dropped.saturating_add(count);
    }
}

/// Where a packet goes among those at its frame, before the order it came
/// in: a MIDI 1.0 parameter number's controllers in the order a data entry
/// needs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(feature = "vst3"),
    expect(dead_code, reason = "VST3 alone hands controllers over apart")
)]
pub(crate) enum Rank {
    /// A registered or assignable parameter's address (controllers 98 to
    /// 101), which its data entry needs.
    Address,
    /// A data entry high byte (controller 6), which its low byte refines.
    HighByte,
    /// Every other packet.
    Rest,
}

/// The block's MIDI in as the exporter hands it over: each packet keyed by
/// its frame, then its rank there, then the order it came in, so no two
/// keys are equal.
#[derive(Debug)]
struct Arrivals {
    packets: Vec<(u64, Event)>,
    /// The packets past the room.
    dropped: u32,
}

impl Arrivals {
    fn with_room(room: usize) -> Self {
        Arrivals {
            packets: Vec::with_capacity(room),
            dropped: 0,
        }
    }

    /// Where the key's rank begins: two bits below the frame.
    const RANK: u32 = 30;

    /// Adds `event` at `rank` while there is room, which never allocates.
    fn push(&mut self, event: Event, rank: Rank) {
        if self.packets.len() < self.packets.capacity() {
            // Below the rank, since a block's room is far smaller.
            let order = u64::try_from(self.packets.len()).unwrap_or(0) & ((1 << Self::RANK) - 1);
            let rank = match rank {
                Rank::Address => 0_u64,
                Rank::HighByte => 1,
                Rank::Rest => 2,
            };
            let key = u64::from(event.offset).wrapping_shl(32) | rank << Self::RANK | order;
            self.packets.push((key, event));
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    fn clear(&mut self) {
        self.packets.clear();
        self.dropped = 0;
    }
}

/// Fills `audio`'s MIDI in for a block of `frames` frames, at least one:
/// what `midi` pushes, in time order, then what the upgrade held to the
/// block's end.
fn fill<R>(audio: &mut Active<R>, frames: usize, midi: impl FnOnce(&mut MidiIn<'_>)) {
    let Active {
        arrivals,
        events,
        upgrade,
        ..
    } = audio;
    arrivals.clear();
    midi(&mut MidiIn {
        arrivals: &mut *arrivals,
    });
    // No two keys are equal, so a sort that need not keep equal ones in
    // order, which sorts in place and allocates nothing, keeps them as
    // they came.
    arrivals.packets.sort_unstable_by_key(|&(key, _)| key);
    events.clear();
    events.add_dropped(arrivals.dropped);
    for (_, event) in &arrivals.packets {
        upgrade.event(event, events);
    }
    let last = u32::try_from(frames.saturating_sub(1)).unwrap_or(u32::MAX);
    upgrade.end_block(last, events);
}

/// What a block found in the audio slot.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Held<T> {
    /// The processor ran the block, which came to this.
    Ran(T),
    /// The processor panicked in the block; it is marked faulted now.
    Panicked,
    /// No processor, one that panicked before, or a slot an activation
    /// holds: nothing ran.
    Idle,
}

/// Runs `block` on the processor in `slot`, when no one else holds the slot,
/// there is a processor and it never panicked.
///
/// A panic in `block` is caught here, while the slot is still held, and
/// marks that processor faulted, so the mark stays with it. An activation,
/// which must wait for the slot to replace the processor, installs one that
/// starts clean, however the two threads interleave.
pub(super) fn run_held<R, T>(
    slot: &AudioSlot<Option<Active<R>>>,
    block: impl FnOnce(&mut Active<R>) -> T,
) -> Held<T> {
    let Some(mut held) = slot.try_hold() else {
        return Held::Idle;
    };
    let Some(audio) = held.as_mut() else {
        return Held::Idle;
    };
    if audio.faulted {
        return Held::Idle;
    }
    match panic::catch_unwind(AssertUnwindSafe(|| block(&mut *audio))) {
        Ok(result) => Held::Ran(result),
        Err(_) => {
            audio.faulted = true;
            Held::Panicked
        }
    }
}

/// A reset asked of the processor in the audio slot, which the next block
/// on it runs first.
pub(super) struct PendingReset(AtomicBool);

impl PendingReset {
    pub(super) fn new() -> Self {
        PendingReset(AtomicBool::new(false))
    }

    /// Asks the next block to reset its processor first. Any thread.
    pub(super) fn ask(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether a reset was asked, clearing the request: for a block that
    /// holds the slot.
    fn take(&self) -> bool {
        self.0.swap(false, Ordering::AcqRel)
    }
}

/// Puts `audio` in `slot` for an activation, returning the processor it
/// replaces. A reset asked of that one is dropped, since the new one has
/// nothing to clear.
///
/// The request is cleared while the slot is held. A block reaches the new
/// processor only by holding the slot after that, so a reset asked once a
/// block has run on it is kept for the next one.
pub(super) fn install<R>(
    slot: &AudioSlot<Option<Active<R>>>,
    reset: &PendingReset,
    audio: Active<R>,
) -> Option<Active<R>> {
    let mut held = slot.hold();
    reset.0.store(false, Ordering::Release);
    held.replace(audio)
}

/// What a block came to, for the shell to answer its host with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// A block of no frames: the host's writes landed and nothing played.
    Empty,
    /// The block did not play: the shell writes silence to every output.
    Silent,
    /// The processor played it, answering this for its last piece.
    Played(Status),
}

impl<P: Plugin, H: HostLink> Engine<P, H> {
    /// One block of `frames` frames.
    ///
    /// First comes the host's automation for it, which `automation` hands over
    /// queue by queue and which lands before the block. Then `midi` hands
    /// over the host's MIDI for it, upgraded as it arrives, and learn's audio
    /// side runs over that, when the plugin's parts have learn on. Then the
    /// processor runs, in pieces when the block is longer than the activation
    /// promised. Then `sent` is handed the MIDI the processor sent, at the
    /// block's frames, for the exporter to pass to its host. A block of no
    /// frames carries no MIDI: `midi` and `sent` are not called, and neither
    /// are they when no processor runs.
    ///
    /// A panic in the processor is caught while it is still held (see
    /// [`run_held`]): the block is silent, and so is every block until the
    /// next activation's processor. A panic anywhere else in the block, the
    /// automation walk say, is caught here and silences that block alone. A
    /// panic in `sent` comes after the block played, so it is counted and
    /// the block stands.
    ///
    /// The calling thread carries the params crate's audio-thread mark
    /// throughout, so a debug build panics, caught as above, at any lock or
    /// audio-side drop the block reaches. Nothing here drops a processor:
    /// one that panicked stays in its slot until the main thread replaces
    /// or takes it. A panic in `midi` silences the block, as one in the
    /// automation walk does, and leaves the processor alone.
    ///
    /// # Safety
    ///
    /// Each non-null pointer in `inputs` is valid for reading, and in
    /// `outputs` for reading and writing, `frames` floats until this call
    /// returns, and nothing else reads or writes them meanwhile. Any two of
    /// them are one buffer or share no float: an output may be the same
    /// buffer as an input, as a host processing in place hands it.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub(crate) unsafe fn process(
        &self,
        frames: usize,
        inputs: &[*mut f32],
        outputs: &[*mut f32],
        automation: impl FnOnce(&mut Points<'_>),
        midi: impl FnOnce(&mut MidiIn<'_>),
        sent: impl FnOnce(&[Event]),
    ) -> Outcome {
        let _audio = audio_thread::enter();
        // SAFETY: this function's contract, passed on whole.
        let block = || unsafe { self.play(frames, inputs, outputs, automation, midi, sent) };
        panic::catch_unwind(AssertUnwindSafe(block)).unwrap_or_else(|_| {
            self.faults.record(Fault::Panic);
            Outcome::Silent
        })
    }

    /// The block, with only the processor's panics caught.
    ///
    /// # Safety
    ///
    /// As [`process`](Self::process).
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    unsafe fn play(
        &self,
        frames: usize,
        inputs: &[*mut f32],
        outputs: &[*mut f32],
        automation: impl FnOnce(&mut Points<'_>),
        midi: impl FnOnce(&mut MidiIn<'_>),
        sent: impl FnOnce(&[Event]),
    ) -> Outcome {
        {
            let mut room = self.automation.try_hold();
            let mut points = Points::new(room.as_deref_mut(), &self.host);
            automation(&mut points);
            points.apply();
        }
        // After the host's writes, as JUCE's wrapper reads them outside the
        // plugin's `processBlock` and its `ScopedNoDenormals`.
        let _flush = FlushDenormals::new();
        if frames == 0 {
            return Outcome::Empty;
        }
        let held = run_held(&self.audio, |audio| {
            if self.reset.take() {
                audio.processor.reset();
            }
            // The exporter's code, not the processor's: a panic in it
            // silences the block without faulting the processor.
            let filled = panic::catch_unwind(AssertUnwindSafe(|| fill(audio, frames, midi)));
            if filled.is_err() {
                return None;
            }
            // SAFETY: this function's contract.
            let status = unsafe { pieces(audio, frames, inputs, outputs) };
            // The exporter's code too: the block has played, so a panic in
            // it is counted and the block stands.
            let out = audio.out_events.as_slice();
            let handed = panic::catch_unwind(AssertUnwindSafe(|| sent(out)));
            Some((status, handed.is_ok()))
        });
        match held {
            Held::Ran(Some((status, handed))) => {
                if !handed {
                    self.faults.record(Fault::Panic);
                }
                Outcome::Played(status)
            }
            Held::Ran(None) => {
                self.faults.record(Fault::Panic);
                Outcome::Silent
            }
            Held::Panicked => {
                self.faults.record(Fault::AudioPanic);
                Outcome::Silent
            }
            Held::Idle => Outcome::Silent,
        }
    }
}

/// The processor over the block, in pieces of at most the activation's
/// frames, answering the last piece's status.
///
/// Learn's audio side runs first, once, over the whole block's events, as the
/// host's automation lands before the block: so every piece reads what a
/// learned controller wrote, and learn counts a host's blocks, which times
/// its wait for the second half of a 14-bit pair. In a block that plays in
/// pieces, a controller in a later piece applies from the block's start.
///
/// The block's MIDI is decoded once, after learn, and each piece is handed
/// the messages and packets that fall in it, at frames from its own start.
/// One past the block's end plays at its last frame, in the last piece:
/// dropping it could lose a note-off and leave a note hanging. The first
/// piece also reports the block's drops. They were lost before any piece
/// ran, at frames no one knows, and the first piece is the earliest a
/// processor can act on them. Reported once, they add up over the pieces to
/// the block's count.
///
/// Every piece sends into the block's list, and what it sent moves to the
/// block's frames once it returns, a frame past the piece's end at its last
/// frame, as MIDI in does: so nothing lands in a later piece or past the
/// block. A later piece finds the earlier pieces' events already there.
///
/// # Safety
///
/// As [`Engine::process`].
unsafe fn pieces<R: Processor>(
    audio: &mut Active<R>,
    frames: usize,
    inputs: &[*mut f32],
    outputs: &[*mut f32],
) -> Status {
    let Active {
        processor,
        channels,
        events,
        midi,
        out_events,
        learn,
        ..
    } = audio;
    if let Some(learn) = learn {
        learn.process(events);
    }
    midi.read(events, frames);
    out_events.clear();
    let mut status = Status::Continue;
    let mut offset = 0;
    while offset < frames {
        // At least a frame, since the activation's frames are never 0, so
        // the loop ends.
        let length = frames
            .saturating_sub(offset)
            .min(channels.max_frames().get());
        let piece = midi.midi().between(offset, offset.saturating_add(length));
        let sent = out_events.as_slice().len();
        // SAFETY: each non-null channel holds `frames` floats (this
        // function's contract), so `offset + length` of them, nothing else
        // touches them during the call, and `length` is at most the
        // scratch's frames.
        status = unsafe {
            channels.run(inputs, outputs, offset, length, |block| {
                let audio = Audio::new(block, length);
                processor.process(Block::new(audio, piece, MidiOut::new(out_events)))
            })
        };
        let start = u32::try_from(offset).unwrap_or(u32::MAX);
        let last = u32::try_from(length.saturating_sub(1)).unwrap_or(u32::MAX);
        for event in out_events.as_mut_slice().iter_mut().skip(sent) {
            event.offset = event.offset.min(last).saturating_add(start);
        }
        offset = offset.saturating_add(length);
    }
    status
}

#[cfg(test)]
pub(super) mod tests;

#[cfg(all(test, loom))]
mod loom_models {
    use loom::sync::Arc;

    use std::num::NonZeroUsize;

    use super::{Active, Held, PendingReset, install, run_held};
    use crate::boundary::AudioSlot;
    use crate::layout::Width;

    /// A processor that says which activation built it.
    #[derive(Debug)]
    struct Built(u32);

    fn audio(activation: u32) -> Option<Active<Built>> {
        Some(Active::around(
            Built(activation),
            Width::new(0, 0),
            NonZeroUsize::MIN,
            0,
            0,
        ))
    }

    /// A block whose processor panics races an activation that replaces
    /// the processor. Whatever the interleaving, the fault marks the
    /// processor the block ran on and no other: the activation's own
    /// starts clean unless the block ran on it.
    #[test]
    fn loom_a_panicking_block_racing_an_activation_marks_only_its_own_processor() {
        loom::model(|| {
            let slot = Arc::new(AudioSlot::new(audio(1)));
            let audio_side = Arc::clone(&slot);
            let block = loom::thread::spawn(move || {
                let mut ran_on = None;
                let held = run_held(&audio_side, |audio: &mut Active<Built>| {
                    ran_on = Some(audio.processor.0);
                    // Unwinds as a panic does, without the panic hook's
                    // message for every interleaving.
                    std::panic::resume_unwind(Box::new(()))
                });
                assert!(matches!(held, Held::Panicked | Held::Idle), "{held:?}");
                ran_on
            });
            let old = slot.hold().replace(audio(2).expect("built"));
            let ran_on = block.join().expect("the audio thread");

            let old = old.expect("the first processor");
            assert_eq!(old.faulted, ran_on == Some(1), "the first processor");
            let new = slot.try_hold().expect("free once both let go");
            let new = new.as_ref().expect("the activation's processor");
            assert_eq!(new.faulted, ran_on == Some(2), "the activation's processor");
        });
    }

    /// A block, then a reset asked once it is done, as a host's stop of
    /// processing asks one, race an activation that installs a processor
    /// and drops the resets asked of the one it replaces. Whatever the
    /// interleaving, once the block ran on the new processor the reset
    /// asked after it is kept for the next block.
    #[test]
    fn loom_a_reset_asked_after_a_block_on_a_new_processor_is_kept() {
        loom::model(|| {
            let slot = Arc::new(AudioSlot::new(audio(1)));
            let reset = Arc::new(PendingReset::new());
            let (audio_side, asked) = (Arc::clone(&slot), Arc::clone(&reset));
            let block = loom::thread::spawn(move || {
                // As a block takes the request before it plays.
                let held = run_held(&audio_side, |audio: &mut Active<Built>| {
                    asked.take();
                    audio.processor.0
                });
                asked.ask();
                held
            });
            let old = install(&slot, &reset, audio(2).expect("built"));
            let held = block.join().expect("the audio thread");

            assert_eq!(old.map(|old| old.processor.0), Some(1));
            if held == Held::Ran(2) {
                assert!(reset.take(), "the reset asked after the new one ran");
            }
        });
    }
}
