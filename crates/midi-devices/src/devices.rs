//! The system's list of MIDI devices, read again when it may have changed.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::backend;
use crate::id::{Device, DeviceId, Direction};

/// The list of the computer's MIDI devices, on the main thread.
///
/// It reads the list at [`new`](Self::new), and again at a
/// [`tick`](Self::tick) when it may have changed, telling what came and went.
/// The system marks it as changed from a thread of its own; the tick reads
/// it half a second after, then one, two and four seconds after each look,
/// since a system's list can lag its own notice. Where the backend cannot be
/// told, the tick reads it every two seconds.
///
/// Open: only WinMM is built, so elsewhere the list is empty.
///
/// # Examples
///
/// ```
/// use hapto_midi_devices::Devices;
///
/// let mut devices = Devices::new();
/// for device in devices.inputs() {
///     println!("{}", device.name());
/// }
/// // Once per tick of the app's main thread.
/// let changes = devices.tick(std::time::Instant::now());
/// for device in &changes.added {
///     println!("came: {}", device.name());
/// }
/// ```
#[derive(Debug)]
pub struct Devices {
    list: Vec<Device>,
    /// Set from the system's thread when the list may have changed.
    marked: Arc<AtomicBool>,
    /// Keeps the system's notice coming; `None` where it cannot.
    watch: Option<backend::Watch>,
    looks: Looks,
}

/// What came and went between two looks at the list.
///
/// # Examples
///
/// ```
/// use hapto_midi_devices::Devices;
///
/// let mut devices = Devices::new();
/// let changes = devices.refresh();
/// for device in &changes.added {
///     println!("came: {}", device.name());
/// }
/// for device in &changes.removed {
///     println!("went: {}", device.name());
/// }
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Changes {
    /// The devices that came, in the system's order.
    pub added: Vec<Device>,
    /// The devices that went, in the order the list had them.
    pub removed: Vec<Device>,
}

impl Changes {
    /// Whether nothing came or went.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
}

impl Default for Devices {
    fn default() -> Self {
        Devices::new()
    }
}

impl Devices {
    /// Reads the system's list and starts watching it. Thread: main.
    pub fn new() -> Devices {
        let marked = Arc::new(AtomicBool::new(false));
        let watch = backend::Watch::new(Arc::clone(&marked));
        Devices {
            list: backend::list(),
            marked,
            looks: Looks::new(watch.is_some()),
            watch,
        }
    }

    /// Every device, inputs then outputs, each in the system's order.
    pub fn all(&self) -> &[Device] {
        &self.list
    }

    /// The devices that send MIDI.
    pub fn inputs(&self) -> impl Iterator<Item = &Device> {
        self.of(Direction::Input)
    }

    /// The devices that take MIDI.
    pub fn outputs(&self) -> impl Iterator<Item = &Device> {
        self.of(Direction::Output)
    }

    fn of(&self, direction: Direction) -> impl Iterator<Item = &Device> {
        self.list
            .iter()
            .filter(move |device| device.direction() == direction)
    }

    /// The device `id` names, if the list has it.
    pub fn get(&self, id: &DeviceId) -> Option<&Device> {
        self.list.iter().find(|device| device.id() == id)
    }

    /// Reads the list again if it may have changed and a look is due by
    /// `now`, and tells what came and went. Call it once per tick.
    ///
    /// Thread: main.
    pub fn tick(&mut self, now: std::time::Instant) -> Changes {
        if self.marked.swap(false, Ordering::Acquire) {
            self.looks.marked(now);
        }
        if self.looks.due(now) {
            self.refresh()
        } else {
            Changes::default()
        }
    }

    /// Reads the list now, and tells what came and went since the last look.
    ///
    /// Thread: main.
    pub fn refresh(&mut self) -> Changes {
        let list = backend::list();
        let changes = changed(&self.list, &list);
        self.list = list;
        changes
    }

    /// Whether the system tells it when the list changes, so it reads the
    /// list only then.
    pub fn is_watching(&self) -> bool {
        self.watch.is_some()
    }
}

/// What came and went from `old` to `new`, by id.
fn changed(old: &[Device], new: &[Device]) -> Changes {
    let has = |list: &[Device], device: &Device| list.iter().any(|d| d.id() == device.id());
    Changes {
        added: new.iter().filter(|d| !has(old, d)).cloned().collect(),
        removed: old.iter().filter(|d| !has(new, d)).cloned().collect(),
    }
}

/// When the list is next read.
#[derive(Debug)]
struct Looks {
    /// The next look, if one is due.
    next: Option<std::time::Instant>,
    /// The wait before the look after it.
    wait: Duration,
    /// The looks left after the next.
    left: u8,
    /// Whether the system tells of changes; if not, a look every two
    /// seconds.
    told: bool,
}

/// The first look after a notice, and the most looks one brings.
const FIRST: Duration = Duration::from_millis(500);
const LOOKS: u8 = 4;
/// How often the list is read when the system cannot tell of changes.
const POLL: Duration = Duration::from_secs(2);

impl Looks {
    fn new(told: bool) -> Looks {
        Looks {
            next: None,
            wait: FIRST,
            left: 0,
            told,
        }
    }

    /// The system said the list changed at `now`: look half a second
    /// later, and three times more, each wait twice the last.
    fn marked(&mut self, now: std::time::Instant) {
        self.next = Some(now + FIRST);
        self.wait = FIRST;
        self.left = LOOKS - 1;
    }

    /// Whether a look is due by `now`, counting it if so.
    fn due(&mut self, now: std::time::Instant) -> bool {
        match self.next {
            None => {
                if !self.told {
                    self.next = Some(now + POLL);
                }
                false
            }
            Some(next) if now < next => false,
            Some(_) => {
                self.next = if self.left > 0 {
                    self.left -= 1;
                    self.wait *= 2;
                    Some(now + self.wait)
                } else if self.told {
                    None
                } else {
                    Some(now + POLL)
                };
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::named;

    #[test]
    fn a_device_that_came_and_one_that_went_are_told_apart_by_id() {
        let entry = |name: &str| (name.to_owned(), name.to_owned());
        let old = named(Direction::Input, vec![entry("Keys"), entry("Pads")]);
        let new = named(Direction::Input, vec![entry("Pads"), entry("Drums")]);
        let changes = changed(&old, &new);
        let names = |list: &[Device]| list.iter().map(|d| d.name().to_owned()).collect::<Vec<_>>();
        assert_eq!(names(&changes.added), ["Drums"]);
        assert_eq!(names(&changes.removed), ["Keys"]);
        assert!(changed(&new, &new).is_empty());
    }

    #[test]
    fn a_notice_brings_four_looks_each_wait_twice_the_last() {
        let start = std::time::Instant::now();
        let at = |millis| start + Duration::from_millis(millis);
        let mut looks = Looks::new(true);
        assert!(!looks.due(at(10_000)), "nothing marked, nothing due");
        looks.marked(start);
        let mut seen = Vec::new();
        for millis in (0..10_000).step_by(50) {
            if looks.due(at(millis)) {
                seen.push(millis);
            }
        }
        // Half a second, then a second, two and four after each look.
        assert_eq!(seen, [500, 1500, 3500, 7500]);
    }

    #[test]
    fn with_no_notice_the_list_is_read_every_two_seconds() {
        let start = std::time::Instant::now();
        let mut looks = Looks::new(false);
        let due: Vec<u64> = (0..=60)
            .map(|tenth| tenth * 100)
            .filter(|&millis| looks.due(start + Duration::from_millis(millis)))
            .collect();
        assert_eq!(due, [2000, 4000, 6000]);
    }
}
