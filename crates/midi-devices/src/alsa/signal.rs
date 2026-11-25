//! What wakes a thread of the backend's: an eventfd another thread sets,
//! which never blocks.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

/// A signal one thread sets and another waits on: an output's, which the
/// audio thread sets once a block has handed the output MIDI, or one that
/// wakes an input's or the watch's thread to end it.
#[derive(Debug)]
pub(crate) struct Signal(Option<OwnedFd>);

impl Signal {
    /// Creates a signal, not set. Where the system has no eventfd to give,
    /// setting it does nothing and [`poll_fd`](Self::poll_fd) is `None`, so
    /// no thread of the backend's starts on it.
    pub(crate) fn new() -> Signal {
        // SAFETY: the call takes no pointer, and returns a new descriptor
        // or -1.
        let fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
        // SAFETY: a descriptor just made, which nothing else holds.
        Signal((fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    /// Sets the signal, which wakes the thread that waits on it. It never
    /// blocks: the eventfd is non-blocking, and a write it refuses because
    /// its count is at its most leaves it set.
    ///
    /// Thread: any.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub(crate) fn set(&self) {
        if let Some(fd) = &self.0 {
            let one: u64 = 1;
            // SAFETY: eight bytes from a live value, to the eventfd this
            // signal owns.
            unsafe { libc::write(fd.as_raw_fd(), (&raw const one).cast(), 8) };
        }
    }

    /// Clears the signal, so the next wait waits for it again.
    pub(super) fn clear(&self) {
        if let Some(fd) = &self.0 {
            let mut count: u64 = 0;
            // SAFETY: eight bytes into a live value, from the eventfd this
            // signal owns; with nothing set it returns at once.
            unsafe { libc::read(fd.as_raw_fd(), (&raw mut count).cast(), 8) };
        }
    }

    /// What a poll waits on for the signal, if it has an eventfd.
    pub(super) fn poll_fd(&self) -> Option<libc::pollfd> {
        self.0.as_ref().map(|fd| libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        })
    }
}
