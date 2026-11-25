//! What wakes a thread of the backend's: an eventfd another thread sets,
//! which never blocks, and the wait on it.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::ptr;
use std::time::Duration;

/// A signal one thread sets and another waits on: an output's, set by the
/// audio thread once a block has handed it MIDI, or the one that ends a
/// reader's thread.
#[derive(Debug)]
pub(crate) struct Signal(Option<OwnedFd>);

impl Signal {
    /// A signal not set. Where the system has no eventfd to give, setting
    /// it does nothing, and its thread wakes only by its own timeout.
    pub(crate) fn new() -> Signal {
        // SAFETY: the call takes no pointer, and answers a new descriptor
        // or -1.
        let fd = unsafe { libc::eventfd(0, libc::EFD_NONBLOCK | libc::EFD_CLOEXEC) };
        // SAFETY: a descriptor just made, which nothing else holds.
        Signal((fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    /// Sets the signal: the thread that waits on it wakes. Thread: any.
    /// Never blocks: the eventfd is non-blocking, and a count at its most
    /// is set already.
    #[cfg_attr(hapto_rtsan, sanitize(realtime = "nonblocking"))]
    pub(crate) fn set(&self) {
        if let Some(fd) = &self.0 {
            let one: u64 = 1;
            // SAFETY: eight bytes from a live value, to the eventfd this
            // signal owns.
            unsafe { libc::write(fd.as_raw_fd(), (&raw const one).cast(), 8) };
        }
    }

    /// Takes what was set, so the next wait waits again.
    pub(super) fn clear(&self) {
        if let Some(fd) = &self.0 {
            let mut count: u64 = 0;
            // SAFETY: eight bytes into a live value, from the eventfd this
            // signal owns; with nothing set it answers at once.
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

/// How long a thread whose signal has no eventfd sleeps between looks.
const NO_SIGNAL_WAIT: Duration = Duration::from_millis(10);

/// Waits until one of `fds` is ready or `timeout` passes, `None` waiting
/// as long as it takes, to within the system's timer slack.
pub(super) fn wait(fds: &mut [libc::pollfd], timeout: Option<Duration>) {
    let timeout = if fds.is_empty() {
        Some(timeout.map_or(NO_SIGNAL_WAIT, |timeout| timeout.min(NO_SIGNAL_WAIT)))
    } else {
        timeout
    };
    let spec = timeout.map(|timeout| libc::timespec {
        tv_sec: libc::time_t::try_from(timeout.as_secs()).unwrap_or(libc::time_t::MAX),
        // Below a second's nanoseconds: it fits a C long of any width.
        tv_nsec: timeout.subsec_nanos() as libc::c_long,
    });
    let spec = spec.as_ref().map_or(ptr::null(), ptr::from_ref);
    let count = libc::nfds_t::try_from(fds.len()).unwrap_or(0);
    // SAFETY: `fds` is valid for its length, the timeout lives through the
    // call, and no signal mask is given.
    unsafe { libc::ppoll(fds.as_mut_ptr(), count, spec, ptr::null()) };
}
