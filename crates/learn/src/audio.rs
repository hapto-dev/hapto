//! Learn's audio entry.

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

use hapto_params::{AudioParams, audio_thread};
use hapto_ump::EventList;

use crate::capture::Stager;
use crate::parse;
use crate::snapshot::Reader;
use crate::table::Route;

/// The audio thread's side of learn: [`process`](Self::process) runs once
/// per block.
///
/// Drop it off the audio thread: it may hold the last reference to the
/// published tables and to the parameters.
///
/// # Panics
///
/// In a build with `debug_assertions`, dropping it panics while the thread
/// holds an [`audio_thread`] mark.
///
/// # Examples
///
/// ```
/// use hapto_learn::{Control, Mapping, Source};
#[doc = include_str!("../doc/setup.rs")]
/// # use hapto_learn::Config;
/// # use hapto_learn::Learn;
/// # let (mut learn, mut audio) = Learn::new(&set, Config::default());
/// learn.map(gain, Mapping::new(Source::any(Control::Cc(7))))?;
///
/// // Whoever holds it calls this once per block, before the parameters
/// // are read: here a block that moves controller 7 to 127.
/// audio.process(&block(&[(7, 127)]));
/// assert_eq!(set.value(gain), 1.0);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct LearnAudio {
    params: AudioParams,
    reader: Reader,
    stager: Stager,
}

impl Drop for LearnAudio {
    fn drop(&mut self) {
        audio_thread::assert_not_here("dropping LearnAudio");
    }
}

impl LearnAudio {
    pub(crate) fn new(params: AudioParams, reader: Reader, stager: Stager) -> Self {
        LearnAudio {
            params,
            reader,
            stager,
        }
    }

    /// Runs learn over one block's MIDI in order, so a target that several
    /// messages reach ends the block at the last one's value.
    ///
    /// - Takes the newest table that the UI side published, without waiting,
    ///   locking, allocating or freeing.
    /// - Writes each mapped parameter in the block that its message arrived
    ///   in, as a learned write, with no gesture, no undo step and no notice
    ///   to the host.
    /// - Stages a mapped UI-tier target's newest value for
    ///   [`Learn::poll`](crate::Learn::poll), and while a target is armed,
    ///   the newest legal message for the poll to learn.
    /// - Never changes the list, so a processor gets every packet as the
    ///   exporter delivered it.
    ///
    /// Learn reads only MIDI 2.0, so MIDI 1.0 must go through
    /// [`Upgrade`](hapto_ump::Upgrade) first, as the exporters' engine does.
    /// The [`LearnAudio`] example shows a block.
    ///
    /// Thread: audio.
    ///
    /// # Panics
    ///
    /// In a build with `debug_assertions`, when a packet is a MIDI 1.0
    /// channel voice message, which learn would skip without a sign.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn process(&mut self, events: &EventList) {
        debug_assert!(
            events
                .as_slice()
                .iter()
                .all(|event| event.midi1_voice().is_none()),
            "learn reads MIDI 2.0 alone: upgrade MIDI 1.0 with hapto_ump::Upgrade \
             before LearnAudio::process"
        );
        let table = self.reader.read();
        self.stager.block(|offers| {
            for event in events.as_slice() {
                let Some(message) = parse::parse(event) else {
                    continue;
                };
                offers.offer(&message);
                table.dispatch(&message, |route, value| match route {
                    Route::Param(target) => self.params.learned_write(*target, value),
                    Route::Staged(staging) => staging.stage(value),
                    Route::RunTime(param) => param.learned_write(value),
                });
            }
        });
    }

    /// Moves to the newest table the UI side published, as
    /// [`process`](Self::process) does at each block, and returns how many
    /// slots it holds: one per mapped target.
    ///
    /// A hook for tests, which have no other way to see the table the audio
    /// side holds.
    #[doc(hidden)]
    pub fn refresh_slots(&mut self) -> usize {
        self.reader.read().len()
    }
}
