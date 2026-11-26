//! A backend thread that waits on a client and a signal. As its worker
//! drops, it tells the thread to end, wakes it and joins it.

use std::fmt;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::signal::Signal;

/// The descriptors that a worker's thread waits on, a client's first and
/// then its signal's.
pub(super) struct Waiter {
    fds: Vec<libc::pollfd>,
    wake: Arc<Signal>,
    stopping: Arc<AtomicBool>,
}

impl Waiter {
    /// Creates a waiter on `fds`, a client's descriptors, and on `wake`, or
    /// returns `None` if `wake` has no eventfd, because nothing could then
    /// wake the thread to end it.
    pub(super) fn new(mut fds: Vec<libc::pollfd>, wake: Arc<Signal>) -> Option<Waiter> {
        fds.push(wake.poll_fd()?);
        Some(Waiter {
            fds,
            wake,
            stopping: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Returns whether the worker's `stop` has begun.
    pub(super) fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }

    /// Waits until the client has an event to read, the signal is set or
    /// `timeout` passes, and returns whether the client has one.
    ///
    /// A `None` timeout never passes, and any other passes to within the
    /// system's timer slack. A signal that woke it is cleared.
    pub(super) fn wait(&mut self, timeout: Option<Duration>) -> bool {
        for fd in &mut self.fds {
            fd.revents = 0;
        }
        let spec = timeout.map(|timeout| libc::timespec {
            tv_sec: libc::time_t::try_from(timeout.as_secs()).unwrap_or(libc::time_t::MAX),
            // Under a second's nanoseconds, so it fits a C long of any width.
            tv_nsec: timeout.subsec_nanos() as libc::c_long,
        });
        let spec = spec.as_ref().map_or(ptr::null(), ptr::from_ref);
        let count = libc::nfds_t::try_from(self.fds.len()).unwrap_or(0);
        // SAFETY: `fds` is valid for its length, the timeout lives through
        // the call, and no signal mask is given.
        unsafe { libc::ppoll(self.fds.as_mut_ptr(), count, spec, ptr::null()) };
        let ready = |fd: &libc::pollfd| fd.revents & libc::POLLIN != 0;
        let Some((signal, client)) = self.fds.split_last() else {
            return false;
        };
        if ready(signal) {
            self.wake.clear();
        }
        client.iter().any(ready)
    }
}

/// A backend thread, which ends when this drops.
pub(super) struct Worker<T> {
    stopping: Arc<AtomicBool>,
    wake: Arc<Signal>,
    thread: Option<JoinHandle<T>>,
}

impl<T: Send + 'static> Worker<T> {
    /// Starts `body(waiter)` on a thread named `name`, or returns `None` if
    /// the thread can't start, which drops what `body` holds.
    pub(super) fn spawn(
        name: &str,
        waiter: Waiter,
        body: impl FnOnce(Waiter) -> T + Send + 'static,
    ) -> Option<Worker<T>> {
        let (stopping, wake) = (Arc::clone(&waiter.stopping), Arc::clone(&waiter.wake));
        let thread = thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || body(waiter))
            .ok()?;
        Some(Worker {
            stopping,
            wake,
            thread: Some(thread),
        })
    }
}

impl<T> Worker<T> {
    /// Tells the thread to end, wakes it, joins it and returns what it
    /// returned, or `None` if it was already stopped or it panicked.
    pub(super) fn stop(&mut self) -> Option<T> {
        let thread = self.thread.take()?;
        self.stopping.store(true, Ordering::Release);
        self.wake.set();
        thread.join().ok()
    }
}

impl<T> Drop for Worker<T> {
    fn drop(&mut self) {
        self.stop();
    }
}

impl<T> fmt::Debug for Worker<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Worker")
            .field("running", &self.thread.is_some())
            .finish_non_exhaustive()
    }
}
