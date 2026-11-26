//! The sequencer's notice that the list changed: a client of the app's own
//! subscribed to the System client's announcements, and the thread that
//! waits on it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::seq::{ANNOUNCE, Caps, Client, Incoming, Parser, Streams, Version};
use super::signal::Signal;
use super::worker::{Waiter, Worker};

/// A watch that marks the device list for the next tick when a client or
/// port comes, goes or changes.
///
/// Its thread ends when the watch drops.
#[derive(Debug)]
pub(crate) struct Watch {
    /// The thread, which ends when it drops.
    _worker: Worker<()>,
}

impl Watch {
    /// Starts watching, setting `marked` at each change, or returns `None`
    /// if the sequencer can't be watched, so [`Devices`](crate::Devices)
    /// falls back to its timer.
    pub(crate) fn new(marked: Arc<AtomicBool>) -> Option<Watch> {
        let client = Client::open(Streams::Input, Version::Legacy)
            .ok()?
            .with_input_room();
        let port = client.port("list", Caps::WRITE | Caps::NO_EXPORT).ok()?;
        client.connect_from(port, ANNOUNCE).ok()?;
        let parser = Parser::new()?;
        let waiter = Waiter::new(client.poll_fds(), Arc::new(Signal::new()))?;
        Worker::spawn("hapto midi list", waiter, move |waiter| {
            watch(client, parser, &marked, waiter);
        })
        .map(|worker| Watch { _worker: worker })
    }
}

/// Runs the watch's thread, which waits for announcements and marks the
/// list at each one that may change it, until the watch drops.
fn watch(mut client: Client, mut parser: Parser, marked: &AtomicBool, mut waiter: Waiter) {
    while !waiter.is_stopping() {
        if !waiter.wait(None) {
            continue;
        }
        client.read(&mut parser, |incoming| match incoming {
            Incoming::Announced(change) if change.lists() => {
                marked.store(true, Ordering::Release);
            }
            // The kernel dropped announcements, which may have changed the
            // list.
            Incoming::Overrun => marked.store(true, Ordering::Release),
            _ => {}
        });
    }
}
