//! The one clock that every device's messages are timestamped on.

use std::sync::OnceLock;
use std::time::Duration;

/// The clock's start, the first time this process reads it.
static START: OnceLock<std::time::Instant> = OnceLock::new();

/// A moment on the clock of every device's messages, in nanoseconds since
/// the process first read it.
///
/// It is the system's monotonic clock, the performance counter on Windows.
/// A reader takes its block's start with [`now`](Self::now), and places each
/// message by how long before that it came.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
///
/// use hapto_midi_devices::Instant;
///
/// let before = Instant::now();
/// let after = Instant::now();
/// assert!(after >= before);
/// let later = Instant::from_nanos(1_500_000).after(Duration::from_millis(2));
/// assert_eq!(later, Instant::from_nanos(3_500_000));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Instant(u64);

impl Instant {
    /// Returns the moment now.
    ///
    /// Thread: any. Allocates nothing.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub fn now() -> Instant {
        let elapsed = START.get_or_init(std::time::Instant::now).elapsed();
        Instant(u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX))
    }

    /// Returns the moment `nanos` nanoseconds after the clock started.
    pub const fn from_nanos(nanos: u64) -> Instant {
        Instant(nanos)
    }

    /// Returns the nanoseconds since the clock started.
    pub const fn nanos(self) -> u64 {
        self.0
    }

    /// Returns the moment `by` later, or the clock's last moment if that is
    /// past it.
    pub const fn after(self, by: Duration) -> Instant {
        let nanos = by.as_nanos();
        let nanos = if nanos > u64::MAX as u128 {
            u64::MAX
        } else {
            nanos as u64
        };
        Instant(self.0.saturating_add(nanos))
    }

    /// Returns how long after `earlier` this is, or zero if it is not later.
    pub const fn since(self, earlier: Instant) -> Duration {
        Duration::from_nanos(self.0.saturating_sub(earlier.0))
    }
}
