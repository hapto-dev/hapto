//! The settings window's Activity log: what the plugin told the host, and
//! what the dev host did to the plugin, each stamped with the time since the
//! run began.
//!
//! Open: a call is stamped when the dev host takes it from its test host, at
//! the next tick, not when the plugin made it, so the time between two calls
//! is good to a tick.

#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::format::Call;

/// How many entries the log keeps for a settings window that opens later.
pub(crate) const KEPT: usize = 1000;

/// What an entry of the log holds.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum What {
    /// A call the plugin made of the host.
    Call(Call),
    /// What the dev host did to the plugin: a reactivation, a new cut,
    /// processing stopped.
    Note(String),
}

/// An entry of the log.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    /// Its number in the run, from 1, so the page takes each entry once.
    pub(crate) number: u64,
    /// When the dev host saw it, from the start of the run.
    pub(crate) at: Duration,
    /// What it holds.
    pub(crate) what: What,
}

/// The log: the latest [`KEPT`] entries.
#[derive(Debug)]
pub(crate) struct Activity {
    start: Instant,
    entries: VecDeque<Entry>,
    count: u64,
}

impl Activity {
    /// An empty log for a run that began at `start`.
    pub(crate) fn new(start: Instant) -> Activity {
        Activity {
            start,
            entries: VecDeque::new(),
            count: 0,
        }
    }

    /// Logs the plugin's `calls`, seen at `now`.
    pub(crate) fn calls(&mut self, calls: Vec<Call>, now: Instant) {
        for call in calls {
            self.push(What::Call(call), now);
        }
    }

    /// Logs what the dev host did, at `now`.
    pub(crate) fn note(&mut self, text: String, now: Instant) {
        self.push(What::Note(text), now);
    }

    /// Returns the number of the last entry logged, 0 for none.
    pub(crate) fn last(&self) -> u64 {
        self.count
    }

    /// Returns the entries kept after the one numbered `after`, in order.
    pub(crate) fn since(&self, after: u64) -> impl Iterator<Item = &Entry> {
        self.entries
            .iter()
            .filter(move |entry| entry.number > after)
    }

    fn push(&mut self, what: What, now: Instant) {
        self.count += 1;
        self.entries.push_back(Entry {
            number: self.count,
            at: now.saturating_duration_since(self.start),
            what,
        });
        if self.entries.len() > KEPT {
            self.entries.pop_front();
        }
    }
}
