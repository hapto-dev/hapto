//! The eventfd that wakes one of the backend's threads, which another thread
//! sets without blocking.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

/// A signal that one thread sets and another waits on.
///
/// The audio thread sets an output's signal when a block hands the output
/// MIDI. An input's or the watch's thread has one that wakes it to end it.
#[derive(Debug)]
pub(crate) struct Signal(Option<OwnedFd>);

impl Signal {
    /// Creates a signal, not set.
    ///
    /// If the system can't create an eventfd, setting the signal does
    /// nothing and [`poll_fd`](Self::poll_fd) returns `None`, so no backend
    /// thread starts on it.
    pub(crate) fn new() -> Signal {
        // SAFETY: the call takes no pointer, and returns a new descriptor
        // or -1.
        let fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
        // SAFETY: a descriptor just made, which nothing else holds.
        Signal((fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    /// Sets the signal, waking the thread that waits on it.
    ///
    /// It never blocks, because the eventfd is non-blocking, and a write
    /// that it refuses with its counter at the maximum leaves it set.
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
            // signal owns. With nothing set, it returns at once.
            unsafe { libc::read(fd.as_raw_fd(), (&raw mut count).cast(), 8) };
        }
    }

    /// Returns what a poll waits on for the signal, if it has an eventfd.
    pub(super) fn poll_fd(&self) -> Option<libc::pollfd> {
        self.0.as_ref().map(|fd| libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        })
    }
}
