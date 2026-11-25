//! The sequencer's notice that the list changed: a client of the app's own
//! subscribed to the System client's announcements, and the thread that
//! waits on it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use super::seq::{ANNOUNCE, CAP_NO_EXPORT, CAP_WRITE, Client, Incoming, Parser, Streams, Version};
use super::signal::{Signal, wait};

/// Watches the sequencer for clients and ports that come, go or change,
/// and marks the list for the next tick when one does. Its thread ends as
/// it drops.
#[derive(Debug)]
pub(crate) struct Watch {
    stopping: Arc<AtomicBool>,
    wake: Arc<Signal>,
    thread: Option<JoinHandle<()>>,
}

impl Watch {
    /// Watches, setting `marked` on each change; `None` where the sequencer
    /// cannot be watched, so the list is read every two seconds instead.
    pub(crate) fn new(marked: Arc<AtomicBool>) -> Option<Watch> {
        let client = Client::open(Streams::Input, Version::Legacy)
            .ok()?
            .with_input_room();
        let port = client.port("list", CAP_WRITE | CAP_NO_EXPORT).ok()?;
        client.connect_from(port, ANNOUNCE).ok()?;
        let parser = Parser::new()?;
        // Without an eventfd the thread could not be woken to end.
        let wake = Arc::new(Signal::new());
        wake.poll_fd()?;
        let stopping = Arc::new(AtomicBool::new(false));
        let (thread_wake, thread_stopping) = (Arc::clone(&wake), Arc::clone(&stopping));
        let thread = thread::Builder::new()
            .name("hapto midi list".to_owned())
            .spawn(move || watch(client, parser, &marked, &thread_wake, &thread_stopping))
            .ok()?;
        Some(Watch {
            stopping,
            wake,
            thread: Some(thread),
        })
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        self.wake.set();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The watch's thread: it waits for announcements and marks the list on
/// each that may change it, until the watch drops.
fn watch(
    mut client: Client,
    mut parser: Parser,
    marked: &AtomicBool,
    wake: &Signal,
    stopping: &AtomicBool,
) {
    let mut fds = client.poll_fds();
    fds.extend(wake.poll_fd());
    while !stopping.load(Ordering::Acquire) {
        wait(&mut fds, None);
        wake.clear();
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
