//! The dev host's audio settings: the sample rate, the most frames a block
//! holds, and how blocks are cut. A host changes the first two by
//! reactivating the plugin, and many hosts cut blocks their own way. The
//! standalone app takes only the cut, as the device asks.

#[cfg(test)]
mod tests;

#[cfg(any(feature = "dev-host", test))]
/// The sample rates the dev host offers, of the ones the device takes: the
/// rates a host runs a plugin at.
pub(crate) const STANDARD_RATES: [u32; 6] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000];

#[cfg(any(feature = "dev-host", test))]
/// The most frames a block holds, as the dev host offers it.
pub(crate) const MAX_BLOCKS: [usize; 8] = [32, 64, 128, 256, 512, 1024, 2048, 4096];

#[cfg(any(feature = "dev-host", test))]
/// The maximum a block plan starts with.
pub(crate) const DEFAULT_MAX: usize = 512;

/// How the plugin's blocks are cut from the device's buffers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocks {
    /// As the device asks for frames, cut at the maximum: what most hosts
    /// do.
    Device,
    /// Every block the maximum.
    Fixed,
    /// Each block a random length from 1 frame to the maximum.
    Random,
    /// A cycle of the lengths that catch a plugin out: 1 frame, none, the
    /// maximum, one past it, and odd lengths between.
    Awkward,
}

#[cfg(any(feature = "dev-host", test))]
impl Blocks {
    /// Every plan, in the order the settings window lists them.
    pub(crate) const ALL: [Blocks; 4] = [
        Blocks::Device,
        Blocks::Fixed,
        Blocks::Random,
        Blocks::Awkward,
    ];

    /// Returns the plan a name in the session or a message names.
    pub(crate) fn named(name: &str) -> Option<Blocks> {
        Blocks::ALL.into_iter().find(|blocks| blocks.name() == name)
    }

    /// Returns the name the session and the settings window's messages use.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Blocks::Device => "device",
            Blocks::Fixed => "fixed",
            Blocks::Random => "random",
            Blocks::Awkward => "awkward",
        }
    }

    /// Returns the name a person reads.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Blocks::Device => "As the device asks",
            Blocks::Fixed => "Always the maximum",
            Blocks::Random => "Random lengths",
            Blocks::Awkward => "Awkward",
        }
    }

    /// Returns the lengths it gives, in a line.
    #[cfg(all(windows, feature = "dev-host"))]
    pub(crate) fn text(self) -> &'static str {
        match self {
            Blocks::Device => "Its buffers, cut at the block size.",
            Blocks::Fixed => "Every block the same length.",
            Blocks::Random => "From 1 frame to the block size.",
            Blocks::Awkward => "1, 0, max, max + 1 and odd ones.",
        }
    }
}

impl Blocks {
    /// Returns its number, for an atomic the audio thread reads.
    pub(crate) fn index(self) -> u8 {
        match self {
            Blocks::Device => 0,
            Blocks::Fixed => 1,
            Blocks::Random => 2,
            Blocks::Awkward => 3,
        }
    }

    /// Returns the plan whose number is `index`; the device's for any other.
    pub(crate) fn from_index(index: u8) -> Blocks {
        match index {
            1 => Blocks::Fixed,
            2 => Blocks::Random,
            3 => Blocks::Awkward,
            _ => Blocks::Device,
        }
    }
}

#[cfg(any(feature = "dev-host", test))]
/// The audio settings the plugin runs with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    /// The sample rate, in hertz: one the device takes.
    pub(crate) rate: u32,
    /// The most frames a block holds, as the plugin is activated for.
    pub(crate) max: usize,
    /// How blocks are cut.
    pub(crate) blocks: Blocks,
}

#[cfg(any(feature = "dev-host", test))]
impl Settings {
    /// Returns the settings a run starts with on a device at `rate`.
    pub(crate) fn starting(rate: u32) -> Settings {
        Settings {
            rate,
            max: DEFAULT_MAX,
            blocks: Blocks::Device,
        }
    }

    /// Reads settings a session kept, as [`write`](Self::write) writes
    /// them, keeping only a rate in `rates` and a maximum the dev host
    /// offers. `None` when they are not all there.
    pub(crate) fn read(text: &str, rates: &[u32]) -> Option<Settings> {
        let mut fields = text.split_whitespace();
        let rate = fields.next()?.parse().ok().filter(|r| rates.contains(r))?;
        let max = fields
            .next()?
            .parse()
            .ok()
            .filter(|m| MAX_BLOCKS.contains(m))?;
        let blocks = Blocks::named(fields.next()?)?;
        Some(Settings { rate, max, blocks })
    }

    /// Writes them for the session: the rate, the maximum and the plan.
    pub(crate) fn write(self) -> String {
        format!("{} {} {}", self.rate, self.max, self.blocks.name())
    }
}

#[cfg(any(feature = "dev-host", test))]
/// Returns the standard rates within any of `ranges`, each the lowest and
/// the highest rate one configuration of the device takes, and the device's
/// own `default` rate, in order.
pub(crate) fn offered(ranges: impl IntoIterator<Item = (u32, u32)>, default: u32) -> Vec<u32> {
    let ranges: Vec<(u32, u32)> = ranges.into_iter().collect();
    let mut rates: Vec<u32> = STANDARD_RATES
        .into_iter()
        .filter(|rate| {
            ranges
                .iter()
                .any(|(low, high)| (low..=high).contains(&rate))
        })
        .chain([default])
        .collect();
    rates.sort_unstable();
    rates.dedup();
    rates
}
