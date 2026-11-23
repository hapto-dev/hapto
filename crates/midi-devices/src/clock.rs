//! The one clock every device's messages are stamped on.

use std::sync::OnceLock;
use std::time::Duration;

/// When the clock started: the first look at it in this process.
static START: OnceLock<std::time::Instant> = OnceLock::new();

/// A moment on the clock every device's messages are stamped on, in
/// nanoseconds since the process first looked at it: the system's monotonic
/// clock, which on Windows is the performance counter.
///
/// A block's reader takes the time its block starts with
/// [`now`](Self::now), and places each message by how long before it the
/// message came.
///
/// # Examples
///
/// ```
/// use hapto_midi_devices::Instant;
///
/// let before = Instant::now();
/// let after = Instant::now();
/// assert!(after >= before);
/// assert_eq!(Instant::from_nanos(1_500_000).after_millis(2), Instant::from_nanos(3_500_000));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Instant(u64);

impl Instant {
    /// Now. Thread: any. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn now() -> Instant {
        let elapsed = START.get_or_init(std::time::Instant::now).elapsed();
        Instant(u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX))
    }

    /// The moment `nanos` nanoseconds after the clock started.
    pub const fn from_nanos(nanos: u64) -> Instant {
        Instant(nanos)
    }

    /// How many nanoseconds after the clock started it is.
    pub const fn nanos(self) -> u64 {
        self.0
    }

    /// The moment `millis` milliseconds later, as a device stamps a message
    /// by the milliseconds since it started.
    pub const fn after_millis(self, millis: u32) -> Instant {
        Instant(self.0.saturating_add(millis as u64 * 1_000_000))
    }

    /// How long after `earlier` it is: zero if it is not after it.
    pub const fn since(self, earlier: Instant) -> Duration {
        Duration::from_nanos(self.0.saturating_sub(earlier.0))
    }
}
