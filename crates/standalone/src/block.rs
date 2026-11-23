//! The audio device's interleaved buffer, played through a plugin in planar
//! blocks cut as a block plan says.
//!
//! The plugin's blocks are cut apart from the device's buffers: each block
//! is rendered ahead into a queue, and the device takes its frames from the
//! queue. The sound is the same whatever the cut, played at most a block
//! late.

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

use hapto_host::Plugin;
use hapto_host::layout::Width;
use hapto_host::standalone::{MAX_CHANNELS, Standalone, StandaloneAudio};

use crate::settings::Blocks;
use crate::signal::{Generator, Signal};

/// The fewest frames of a device's buffer a part takes: a longer buffer is
/// filled in parts, each as long as this or the most frames a block holds,
/// whichever is more, so a block cut as the device asks reaches the most.
const PART: usize = 1024;

/// How many of the latest blocks' lengths a [`Plan`] keeps.
pub(crate) const RECENT: usize = 40;

/// What the app plays its blocks through: a plugin's audio side, in any
/// format.
pub(crate) trait Render: Send + 'static {
    /// Plays one block, each slice a channel, all as long. A block may hold
    /// no frames.
    fn render(&mut self, inputs: &[&[f32]], outputs: &mut [&mut [f32]]);
}

impl Render for Box<dyn Render> {
    fn render(&mut self, inputs: &[&[f32]], outputs: &mut [&mut [f32]]) {
        (**self).render(inputs, outputs);
    }
}

impl<P: hapto_host::Plugin> Render for StandaloneAudio<P> {
    fn render(&mut self, inputs: &[&[f32]], outputs: &mut [&mut [f32]]) {
        self.process(inputs, outputs, &[]);
    }
}

/// What the main thread may change while blocks play, the cut and the test
/// signal, and the blocks played, which it reads.
#[derive(Debug)]
pub(crate) struct Plan {
    blocks: AtomicU8,
    signal: AtomicU8,
    played: AtomicU64,
    /// The latest blocks' lengths, the block numbered `n` at `n % RECENT`.
    recent: [AtomicU32; RECENT],
}

impl Plan {
    /// A plan that cuts blocks as `blocks` says, with `signal` on the inputs.
    pub(crate) fn new(blocks: Blocks, signal: Signal) -> Arc<Plan> {
        Arc::new(Plan {
            blocks: AtomicU8::new(blocks.index()),
            signal: AtomicU8::new(signal.index()),
            played: AtomicU64::new(0),
            recent: std::array::from_fn(|_| AtomicU32::new(0)),
        })
    }

    /// Cuts the next blocks as `blocks` says. Any thread.
    #[cfg(any(feature = "dev-host", test))]
    pub(crate) fn set(&self, blocks: Blocks) {
        self.blocks.store(blocks.index(), Ordering::Relaxed);
    }

    /// Returns how blocks are cut now.
    pub(crate) fn blocks(&self) -> Blocks {
        Blocks::from_index(self.blocks.load(Ordering::Relaxed))
    }

    /// Plays `signal` into the next blocks. Any thread.
    #[cfg(any(feature = "dev-host", test))]
    pub(crate) fn set_signal(&self, signal: Signal) {
        self.signal.store(signal.index(), Ordering::Relaxed);
    }

    /// Returns the signal the next blocks play.
    pub(crate) fn signal(&self) -> Signal {
        Signal::from_index(self.signal.load(Ordering::Relaxed))
    }

    /// Returns how many blocks were played.
    #[cfg(any(feature = "dev-host", test))]
    pub(crate) fn played(&self) -> u64 {
        // Pairs with the store in `count`, so each length counted is there.
        self.played.load(Ordering::Acquire)
    }

    /// Returns the latest blocks' lengths, up to [`RECENT`], the last one
    /// last. Read while blocks play, the oldest may be a newer block's.
    #[cfg(any(feature = "dev-host", test))]
    pub(crate) fn recent(&self) -> Vec<u32> {
        let played = self.played();
        let kept = played.min(RECENT as u64);
        (played - kept..played)
            .map(|n| self.recent[(n % RECENT as u64) as usize].load(Ordering::Relaxed))
            .collect()
    }

    fn count(&self, length: usize) {
        let length = u32::try_from(length).unwrap_or(u32::MAX);
        let n = self.played.load(Ordering::Relaxed);
        self.recent[(n % RECENT as u64) as usize].store(length, Ordering::Relaxed);
        self.played.store(n + 1, Ordering::Release);
    }
}

/// Where a plan is in cutting blocks: the awkward cycle's place and the
/// random plan's state.
#[derive(Debug)]
struct Cut {
    /// Where the awkward plan is in its cycle.
    cycle: usize,
    /// The random plan's state: a 32-bit xorshift, never 0.
    random: u32,
}

impl Cut {
    fn new() -> Cut {
        Cut {
            cycle: 0,
            random: 0x2545_f491,
        }
    }

    /// Returns the next block's length as `blocks` cuts it, of at most
    /// `max` frames, at least 1, or one past it for the awkward plan;
    /// `needed` is the frames the device still waits for. Every plan gives
    /// a block of frames within a few blocks.
    fn next(&mut self, blocks: Blocks, max: usize, needed: usize) -> usize {
        match blocks {
            Blocks::Device => needed.min(max),
            Blocks::Fixed => max,
            Blocks::Random => {
                let mut x = self.random;
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                self.random = x;
                1 + x as usize % max
            }
            Blocks::Awkward => {
                // Each one past the maximum at most, which the buffers hold.
                let cycle = [
                    1,
                    0,
                    max,
                    3.min(max),
                    max + 1,
                    max - 1,
                    17.min(max),
                    0,
                    max / 2 + 1,
                ];
                let length = cycle[self.cycle % cycle.len()];
                self.cycle = self.cycle.wrapping_add(1);
                length
            }
        }
    }
}

/// The first `count` lengths `blocks` cuts, of at most `max` frames, from a
/// device that asks for `buffer` frames at a time, as a player cuts them:
/// what the settings window draws for each plan.
#[cfg(any(feature = "dev-host", test))]
pub(crate) fn preview(blocks: Blocks, max: usize, buffer: usize, count: usize) -> Vec<usize> {
    let max = max.max(1);
    let mut cut = Cut::new();
    let mut needed = 0;
    (0..count)
        .map(|_| {
            if needed == 0 {
                needed = buffer;
            }
            let length = cut.next(blocks, max, needed);
            needed -= length.min(needed);
            length
        })
        .collect()
}

impl<P: Plugin> Player<StandaloneAudio<P>> {
    /// The shipped app's player: an audio side of `plugin`, laid out as
    /// `layout`, to a device of `device_channels` at `sample_rate`, in blocks
    /// of up to `max` frames cut as `plan` says. The app and its allocation
    /// gate both make it here, so the gate runs what the app plays.
    pub(crate) fn of_app(
        plugin: &Standalone<P>,
        layout: Width,
        device_channels: usize,
        sample_rate: f64,
        max: usize,
        plan: Arc<Plan>,
    ) -> Self {
        Player::new(
            plugin.audio(),
            layout,
            device_channels,
            sample_rate,
            max,
            plan,
        )
    }
}

/// Plays a device's buffers through a [`Render`], with the test signal on
/// every input, in blocks of up to `max` frames cut as its [`Plan`] says.
///
/// Device channel `c` takes the plugin's output `c`. A plugin with one output
/// plays it on every channel, and a channel the plugin has no output for is
/// silent. It allocates nothing once made.
#[derive(Debug)]
pub(crate) struct Player<R> {
    render: R,
    signal: Generator,
    device_channels: usize,
    max: usize,
    /// The most frames of a device's buffer taken at once.
    part: usize,
    plan: Arc<Plan>,
    cut: Cut,
    /// One block's channels, room for one past the maximum.
    inputs: Vec<Vec<f32>>,
    outputs: Vec<Vec<f32>>,
    /// The frames rendered and not yet played, per output.
    queue: Vec<Vec<f32>>,
    queued: usize,
}

impl<R: Render> Player<R> {
    /// Plays through `render`, laid out as `layout`, to a device of
    /// `device_channels` channels at `sample_rate`, in blocks of up to `max`
    /// frames. A layout wider than [`MAX_CHANNELS`] plays its first ones.
    pub(crate) fn new(
        render: R,
        layout: Width,
        device_channels: usize,
        sample_rate: f64,
        max: usize,
        plan: Arc<Plan>,
    ) -> Self {
        let max = max.max(1);
        let channels =
            |count: usize, frames: usize| vec![vec![0.0; frames]; count.min(MAX_CHANNELS)];
        Player {
            render,
            signal: Generator::new(plan.signal(), sample_rate),
            device_channels: device_channels.max(1),
            max,
            part: PART.max(max),
            plan,
            cut: Cut::new(),
            inputs: channels(layout.inputs, max + 1),
            outputs: channels(layout.outputs, max + 1),
            // A block is cut only while the queue is short of a part, and
            // holds up to one past the maximum.
            queue: channels(layout.outputs, PART.max(max) + max + 1),
            queued: 0,
        }
    }

    /// Fills `data`, the device's interleaved buffer, with the plugin's
    /// output for the next frames.
    pub(crate) fn play(&mut self, data: &mut [f32]) {
        let width = self.device_channels;
        for part in data.chunks_mut(self.part * width) {
            let frames = part.len() / width;
            while self.queued < frames {
                let length = self
                    .cut
                    .next(self.plan.blocks(), self.max, frames - self.queued);
                self.play_block(length);
            }
            self.interleave(part, frames);
            self.dequeue(frames);
        }
    }

    /// Renders a block of `length` frames onto the queue.
    fn play_block(&mut self, length: usize) {
        self.signal.set(self.plan.signal());
        if let Some((first, rest)) = self.inputs.split_first_mut() {
            let first = &mut first[..length];
            self.signal.fill(first);
            for channel in rest {
                channel[..length].copy_from_slice(first);
            }
        }
        let (ins, outs) = (self.inputs.len(), self.outputs.len());
        let mut inputs: [&[f32]; MAX_CHANNELS] = [&[]; MAX_CHANNELS];
        for (slot, channel) in inputs.iter_mut().zip(&self.inputs) {
            *slot = &channel[..length];
        }
        let mut outputs: [&mut [f32]; MAX_CHANNELS] = std::array::from_fn(|_| &mut [][..]);
        for (slot, channel) in outputs.iter_mut().zip(&mut self.outputs) {
            *slot = &mut channel[..length];
        }
        self.render.render(&inputs[..ins], &mut outputs[..outs]);
        let at = self.queued;
        for (queue, output) in self.queue.iter_mut().zip(&self.outputs) {
            queue[at..at + length].copy_from_slice(&output[..length]);
        }
        self.queued += length;
        self.plan.count(length);
    }

    fn interleave(&self, part: &mut [f32], frames: usize) {
        let mono = self.queue.len() == 1;
        for (frame, samples) in part
            .chunks_mut(self.device_channels)
            .take(frames)
            .enumerate()
        {
            for (channel, sample) in samples.iter_mut().enumerate() {
                let source = if mono { 0 } else { channel };
                *sample = self
                    .queue
                    .get(source)
                    .and_then(|queue| queue.get(frame))
                    .copied()
                    .unwrap_or(0.0);
            }
        }
    }

    /// Drops the first `frames` of the queue, which the device has played.
    fn dequeue(&mut self, frames: usize) {
        for queue in &mut self.queue {
            queue.copy_within(frames..self.queued, 0);
        }
        self.queued -= frames;
    }
}
