//! The saved block a run starts with, and what the plugin made of it: the
//! settings window's State tab shows it, and a block the plugin refused, or
//! one that could not be read, is held as it was, for the next run to try,
//! rather than replaced by the plugin's defaults.

#[cfg(test)]
mod tests;

use anyhow::Result;

use super::hosted::Hosted;

/// How the run's first load of the saved block went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FirstLoad {
    /// The session held no block.
    Nothing,
    /// The plugin took the block. `upgraded` holds the plugin's state
    /// versions before and after, when its migration moved it on.
    Loaded {
        /// The `pluginStateVersion` the block had and has now.
        upgraded: Option<(u64, u64)>,
    },
    /// The plugin refused the block, as the format answered.
    Refused {
        /// The format's answer.
        reason: String,
        /// The block, which the session keeps unchanged.
        block: Vec<u8>,
    },
    /// The session's block could not be read, for `reason`; the session
    /// leaves it as it is.
    Unreadable {
        /// Why it could not be read.
        reason: String,
    },
}

impl FirstLoad {
    /// Loads `saved`, the session's block as it was read, into `hosted`,
    /// and returns how it went.
    pub(crate) fn of(saved: Result<Option<Vec<u8>>>, hosted: &dyn Hosted) -> FirstLoad {
        let block = match saved {
            Ok(Some(block)) => block,
            Ok(None) => return FirstLoad::Nothing,
            Err(reason) => {
                return FirstLoad::Unreadable {
                    reason: format!("{reason:#}"),
                };
            }
        };
        if let Err(reason) = hosted.load(&block) {
            return FirstLoad::Refused {
                reason: format!("{reason:#}"),
                block,
            };
        }
        let before = plugin_version(&block);
        let after = hosted.save().as_deref().and_then(plugin_version);
        let upgraded = before.zip(after).filter(|(before, after)| after > before);
        FirstLoad::Loaded { upgraded }
    }

    /// Whether the session is to [hold](crate::session::Session::hold) its
    /// block rather than keep the plugin's: after a refusal, or a block that
    /// could not be read, the plugin holds its defaults, not the author's
    /// state.
    pub(crate) fn holds(&self) -> bool {
        matches!(
            self,
            FirstLoad::Refused { .. } | FirstLoad::Unreadable { .. }
        )
    }

    /// Returns what the Activity log says of it.
    pub(crate) fn note(&self) -> String {
        match self {
            FirstLoad::Nothing => "no saved block to load".to_owned(),
            FirstLoad::Loaded { upgraded: None } => "loaded the saved block".to_owned(),
            FirstLoad::Loaded {
                upgraded: Some((before, after)),
            } => format!("loaded the saved block, upgraded from version {before} to {after}"),
            FirstLoad::Refused { reason, .. } => format!("the saved block was refused: {reason}"),
            FirstLoad::Unreadable { reason } => {
                format!("the saved block could not be read: {reason}")
            }
        }
    }
}

/// Returns a block's `pluginStateVersion`: 0 when it has none, as a load
/// reads it, and `None` for a block that is not a JSON object.
fn plugin_version(block: &[u8]) -> Option<u64> {
    let value: serde_json::Value = serde_json::from_slice(block).ok()?;
    let object = value.as_object()?;
    Some(
        object
            .get("pluginStateVersion")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    )
}
