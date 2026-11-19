//! What each parameter was learned from: the MIDI learn mappings the plugin
//! keeps in its saved block, as the settings window's Parameters tab shows
//! them.

#[cfg(test)]
mod tests;

use hapto_learn::SESSION_KEY;
use serde_json::Value as Json;

use super::hosted::Param;

/// Returns, for each of `params`, the sources the saved `block` maps to it,
/// as `group 1 · ch 1 · CC 74`, joined by commas; empty for none. A mapping
/// names its parameter by key, which each parameter's host id is the hash
/// of.
pub(crate) fn learned(block: &[u8], params: &[Param]) -> Vec<String> {
    let mut learned = vec![Vec::new(); params.len()];
    let block: Json = serde_json::from_slice(block).unwrap_or_default();
    let mappings = block["extra"][SESSION_KEY]["mappings"].as_array();
    for mapping in mappings.into_iter().flatten() {
        let (Some(target), Some(source)) = (mapping["target"].as_str(), source(mapping)) else {
            continue;
        };
        let id = hapto_params::host_id(target);
        if let Some(index) = params.iter().position(|p| p.host_id == id) {
            learned[index].push(source);
        }
    }
    learned
        .into_iter()
        .map(|sources| sources.join(", "))
        .collect()
}

/// A mapping's source in words; `None` for an entry this build cannot read.
fn source(mapping: &Json) -> Option<String> {
    let number = mapping["number"].as_u64()?;
    let control = match mapping["kind"].as_str()? {
        "cc" => format!("CC {number}"),
        "nrpn" => format!("NRPN {number}"),
        "note" => format!("note {number}"),
        _ => return None,
    };
    let field = |name: &str, any: &str, prefix: &str| match mapping[name].as_i64()? {
        -1 => Some(any.to_owned()),
        n => Some(format!("{prefix} {}", n.checked_add(1)?)),
    };
    let group = field("group", "any group", "group")?;
    let channel = field("channel", "any channel", "ch")?;
    Some(format!("{group} · {channel} · {control}"))
}
