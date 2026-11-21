//! The app's audio paths, for this crate's allocation gate,
//! `tests/alloc_gate.rs`, alone: hidden, no part of the crate's API, and
//! free to change. Each plays a device's buffers as the device's callback
//! does, on the calling thread, so the gate counts what it allocates.

use std::fmt;

use hapto_host::standalone::Standalone;
use hapto_host::{Layout, Plugin};

pub use crate::settings::Blocks;
pub use crate::signal::Signal;

use crate::block::{Plan, Player};

/// What fills a device's buffer: a player the callback owns, of whatever
/// audio side.
type Play = Box<dyn FnMut(&mut [f32])>;

/// A device's callback, as the app hands it to the device.
pub struct Callback {
    play: Play,
}

impl fmt::Debug for Callback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Callback").finish_non_exhaustive()
    }
}

impl Callback {
    /// The shipped app's, over an audio side of `plugin`, which the caller
    /// activated for `layout` at `sample_rate` and blocks of up to `max`:
    /// to a device of `device_channels`, cutting blocks as `blocks` says,
    /// with `signal` on the inputs.
    pub fn app<P: Plugin>(
        plugin: &Standalone<P>,
        layout: Layout,
        device_channels: usize,
        sample_rate: f64,
        max: usize,
        blocks: Blocks,
        signal: Signal,
    ) -> Callback {
        let plan = Plan::new(blocks, signal);
        let mut player = Player::of_app(plugin, layout, device_channels, sample_rate, max, plan);
        Callback {
            play: Box::new(move |data| player.play(data)),
        }
    }

    /// Fills `data`, the device's interleaved buffer.
    pub fn play(&mut self, data: &mut [f32]) {
        (self.play)(data);
    }
}

/// The dev host's callback, with the plugin it plays in one format.
#[cfg(all(windows, feature = "dev-host"))]
pub mod dev {
    use std::fmt;
    use std::sync::Arc;

    use hapto_host::Plugin;

    pub use crate::dev_host::Format;

    use super::{Blocks, Callback, Signal};
    use crate::block::{Plan, Player};
    use crate::dev_host::{Hosted, host};

    /// The dev host's callback, with the plugin it plays in one format,
    /// started as the dev host starts it. It stops the plugin as it drops.
    pub struct DevHost {
        callback: Option<Callback>,
        plan: Arc<Plan>,
        hosted: Box<dyn Hosted>,
    }

    impl fmt::Debug for DevHost {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("DevHost")
                .field("format", &self.hosted.format())
                .finish_non_exhaustive()
        }
    }

    impl DevHost {
        /// Makes `P` in `format`, activated in its first layout at
        /// `sample_rate` for blocks of up to `max` and processing, playing to
        /// a device of `device_channels`, cut as the device asks, in silence.
        ///
        /// # Errors
        ///
        /// Returns which call the plugin refused.
        pub fn start<P: Plugin>(
            format: Format,
            device_channels: usize,
            sample_rate: f64,
            max: usize,
        ) -> anyhow::Result<DevHost> {
            let hosted = host::<P>(format);
            let layout = crate::app::first_layout::<P>();
            let render = hosted.start(sample_rate, max, layout)?;
            let plan = Plan::new(Blocks::Device, Signal::Silence);
            let mut player = Player::new(
                render,
                layout,
                device_channels,
                sample_rate,
                max,
                Arc::clone(&plan),
            );
            Ok(DevHost {
                callback: Some(Callback {
                    play: Box::new(move |data| player.play(data)),
                }),
                plan,
                hosted,
            })
        }

        /// Fills `data`, the device's interleaved buffer.
        pub fn play(&mut self, data: &mut [f32]) {
            if let Some(callback) = &mut self.callback {
                callback.play(data);
            }
        }

        /// Cuts the next blocks as `blocks` says, as the settings window
        /// does.
        pub fn cut(&self, blocks: Blocks) {
            self.plan.set(blocks);
        }

        /// Plays `signal` into the next blocks, as the settings window does.
        pub fn play_into(&self, signal: Signal) {
            self.plan.set_signal(signal);
        }

        /// Sets the parameter at `index`, 0 to 1, as the settings window
        /// does: while processing, the next block applies it.
        pub fn set(&self, index: usize, normalized: f64) {
            self.hosted.set(index, normalized);
        }

        /// Returns how many faults the instance has had.
        pub fn faults(&self) -> u64 {
            self.hosted.faults()
        }
    }

    impl Drop for DevHost {
        fn drop(&mut self) {
            // As the dev host stops: the device's stream first, then the
            // plugin.
            drop(self.callback.take());
            self.hosted.stop();
        }
    }
}
