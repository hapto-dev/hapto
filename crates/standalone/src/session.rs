//! What the app keeps between runs, in a folder of its own: the plugin's
//! saved block, and for the dev host the format it runs the plugin through.
//! Under `cargo hapto dev` the folder is the one the command names, and the
//! app's exit code [`RELAUNCH`] asks the command to start it again;
//! otherwise it is the plugin's own folder below the user's local
//! application data.

#[cfg(test)]
mod tests;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

/// The exit code that asks `cargo hapto dev` to start the app again at once,
/// as a switch of the dev host's format does. `cargo-hapto` reads it as its
/// own `RELAUNCH`, which a test of its holds to this line.
#[cfg(all(windows, feature = "dev-host"))]
pub(crate) const RELAUNCH: u8 = 3;

/// The saved block's file: the block is JSON.
const SAVED: &str = "saved.json";
/// The dev host's format's file.
#[cfg(feature = "dev-host")]
const FORMAT: &str = "format";
/// The dev host's audio settings' file.
#[cfg(feature = "dev-host")]
const AUDIO: &str = "audio";

/// The folder the app keeps its session in, and what it knows of the saved
/// block there.
#[derive(Debug)]
pub(crate) struct Session {
    dir: PathBuf,
    kept: Kept,
}

/// What a session knows of the saved block in its folder.
#[derive(Debug, PartialEq, Eq)]
enum Kept {
    /// Nothing read or written yet.
    Nothing,
    /// The block last read or written, so an unchanged one is not written.
    Block(Vec<u8>),
    /// The block there stays as it is: the plugin refused it, or it could
    /// not be read. The plugin holds its defaults then, not the author's
    /// state, and the block waits for a build that loads it.
    Held,
}

impl Session {
    /// Opens the session in `dev`, or in the plugin's own folder below the
    /// user's local application data, named by its `id`, and makes the
    /// folder.
    ///
    /// # Errors
    ///
    /// Returns why the folder could not be made.
    pub(crate) fn open(id: &str, dev: Option<&Path>) -> Result<Session> {
        let dir = match dev {
            Some(dir) => dir.to_path_buf(),
            None => std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .context("LOCALAPPDATA is not set")?
                .join(id)
                .join("Standalone"),
        };
        Session::in_dir(dir)
    }

    /// Opens the session in `dir`, and makes the folder.
    ///
    /// # Errors
    ///
    /// Returns why the folder could not be made.
    pub(crate) fn in_dir(dir: PathBuf) -> Result<Session> {
        fs::create_dir_all(&dir).with_context(|| dir.display().to_string())?;
        Ok(Session {
            dir,
            kept: Kept::Nothing,
        })
    }

    /// Returns the saved block the session holds, `None` when it has none.
    ///
    /// # Errors
    ///
    /// Returns why a block that is there could not be read. The session then
    /// [holds](Self::hold) it.
    pub(crate) fn saved_block(&mut self) -> Result<Option<Vec<u8>>> {
        let path = self.dir.join(SAVED);
        match fs::read(&path) {
            Ok(block) => {
                self.kept = Kept::Block(block.clone());
                Ok(Some(block))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => {
                self.hold();
                Err(anyhow::Error::new(error).context(path.display().to_string()))
            }
        }
    }

    /// Keeps the session's saved block as it is from now on, for a plugin
    /// that refused it: [`keep`](Self::keep) writes nothing more.
    pub(crate) fn hold(&mut self) {
        self.kept = Kept::Held;
    }

    /// Keeps `block` as the session's saved block, unless it is the one kept
    /// already, or the session [holds](Self::hold) its own. It is written
    /// [whole](write_whole), so a run that ends mid-write leaves the old one.
    ///
    /// # Errors
    ///
    /// Returns why it could not be written.
    pub(crate) fn keep(&mut self, block: Vec<u8>) -> Result<()> {
        match &self.kept {
            Kept::Held => return Ok(()),
            Kept::Block(kept) if *kept == block => return Ok(()),
            Kept::Block(_) | Kept::Nothing => {}
        }
        write_whole(&self.dir, SAVED, &block)?;
        self.kept = Kept::Block(block);
        Ok(())
    }

    /// Returns the path of `name` in the session's folder.
    #[cfg(all(windows, feature = "dev-host"))]
    pub(crate) fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// Returns the format the dev host last ran the plugin through, by name.
    #[cfg(feature = "dev-host")]
    pub(crate) fn format(&self) -> Option<String> {
        let name = fs::read_to_string(self.dir.join(FORMAT)).ok()?;
        Some(name.trim().to_owned())
    }

    /// Keeps the format the dev host runs the plugin through next.
    ///
    /// # Errors
    ///
    /// Returns why it could not be written.
    #[cfg(feature = "dev-host")]
    pub(crate) fn set_format(&self, name: &str) -> Result<()> {
        write_whole(&self.dir, FORMAT, name.as_bytes())
    }

    /// Returns the dev host's audio settings as it last kept them.
    #[cfg(feature = "dev-host")]
    pub(crate) fn audio(&self) -> Option<String> {
        fs::read_to_string(self.dir.join(AUDIO)).ok()
    }

    /// Keeps the dev host's audio settings for the next run.
    ///
    /// # Errors
    ///
    /// Returns why they could not be written.
    #[cfg(feature = "dev-host")]
    pub(crate) fn set_audio(&self, text: &str) -> Result<()> {
        write_whole(&self.dir, AUDIO, text.as_bytes())
    }
}

/// Writes `bytes` as the file `name` in `dir`, whole or not at all: beside
/// it first, on the disk, then moved over it, so a run or a machine that
/// stops mid-write leaves the old file as it was.
///
/// # Errors
///
/// Returns why it could not be written.
fn write_whole(dir: &Path, name: &str, bytes: &[u8]) -> Result<()> {
    let path = dir.join(name);
    let fresh = dir.join(format!("{name}.new"));
    let written = (|| {
        let mut file = fs::File::create(&fresh)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&fresh, &path)
    })();
    written.with_context(|| path.display().to_string())
}
