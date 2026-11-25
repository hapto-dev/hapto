//! The sequencer's notice that the list changed: a client of the app's own
//! subscribed to the System client's announcements, and the thread that
//! waits on it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::seq::{ANNOUNCE, Caps, Client, Incoming, Parser, Streams, Version};
use super::signal::Signal;
use super::worker::{Waiter, Worker};

/// Watches the sequencer for clients and ports that come, go or change,
/// and marks the list for the next tick when one does. Its thread ends as
/// it drops.
#[derive(Debug)]
pub(crate) struct Watch {
    /// Ends the thread as it drops.
    _worker: Worker<()>,
}

impl Watch {
    /// Watches, setting `marked` on each change, or returns `None` where
    /// the sequencer cannot be watched, and [`Devices`](crate::Devices)
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

/// Runs the watch's thread: waits for announcements and marks the list on
/// each that may change it, until the watch drops.
fn watch(mut client: Client, mut parser: Parser, marked: &AtomicBool, mut waiter: Waiter) {
    while !waiter.is_stopping() {
        if !waiter.wait(None) {
            continue;
        }
        client.read(&mut parser, |incoming| match incoming {
            Incoming::Announced(change) if change.lists() => {
                marked.store(true, Ordering::Release);
            }
            // Announcements were dropped, any of which may have changed
            // the list.
            Incoming::Overrun => marked.store(true, Ordering::Release),
            _ => {}
        });
    }
}
