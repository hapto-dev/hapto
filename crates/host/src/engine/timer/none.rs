//! No thread timer, on a system whose threads have no event loop that Hapto
//! drives, and under Miri. Linux's VST3 hosts offer their run loop instead.

use super::Tick;

/// A thread timer, which can't exist here.
#[derive(Debug)]
pub(crate) enum Timer {}

/// A thread timer's handle, which can't exist either.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Handle {}

impl Timer {
    /// Returns `None`, because there is no event loop to tick on.
    pub(super) fn start(_: Tick) -> Option<Timer> {
        None
    }

    pub(super) fn handle(&self) -> Handle {
        match *self {}
    }
}

impl Handle {
    pub(super) fn detach(self) {
        match self {}
    }

    #[cfg(test)]
    pub(super) fn is_detached(self) -> bool {
        match self {}
    }
}

/// Does nothing, because no timer registered anything.
pub(super) fn unregister() {}

/// Waits out `period`, because there is no loop to run.
#[cfg(any(test, feature = "test-host"))]
pub(super) fn pump(period: std::time::Duration) {
    std::thread::sleep(period);
}
