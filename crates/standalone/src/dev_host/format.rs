//! The formats the dev host runs a plugin through, and what the plugin tells
//! the host in each.

#[cfg(test)]
mod tests;

/// A plugin format the dev host runs the plugin through, in process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// The VST3 exporter, through its COM interfaces, with its own timer.
    Vst3,
    /// The C interface the JUCE adapter drives, ticked by the dev host.
    Cabi,
}

impl Format {
    /// Every format, in the order the settings window lists them.
    pub(crate) const ALL: [Format; 2] = [Format::Vst3, Format::Cabi];

    /// Returns the format a name in the session or a message names.
    pub(crate) fn named(name: &str) -> Option<Format> {
        Format::ALL.into_iter().find(|format| format.name() == name)
    }

    /// Returns the name the session and the settings window's messages use.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Format::Vst3 => "vst3",
            Format::Cabi => "cabi",
        }
    }

    /// Returns the name a person reads.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Format::Vst3 => "VST3",
            Format::Cabi => "C interface",
        }
    }
}

/// A call the plugin made of the host, as the settings window's Activity
/// log shows it: the call's name in the format, and what it carried, each a
/// label and a value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Call {
    /// The call's name in the format: `performEdit`, `perform_edit`.
    pub(crate) name: &'static str,
    /// What it carried: the parameter by its title, its value as 0 to 1, a
    /// restart's flags.
    pub(crate) fields: Vec<(&'static str, String)>,
}

impl Call {
    /// A call about the parameter titled `title`, carrying its `value` as 0
    /// to 1 where it carries one.
    pub(crate) fn edit(name: &'static str, title: Option<&str>, value: Option<f64>) -> Call {
        let mut fields = Vec::new();
        if let Some(title) = title {
            fields.push(("Param", title.to_owned()));
        }
        if let Some(value) = value {
            fields.push(("Value", format!("{value:.4}")));
        }
        Call { name, fields }
    }

    /// A restart, with `flags` named as `known` names its bits, and any
    /// other bits in hex.
    pub(crate) fn restart(name: &'static str, flags: u64, known: &[(u64, &str)]) -> Call {
        let mut words: Vec<String> = known
            .iter()
            .filter(|(bit, _)| flags & bit != 0)
            .map(|(_, word)| (*word).to_owned())
            .collect();
        let rest = known.iter().fold(flags, |rest, (bit, _)| rest & !bit);
        if rest != 0 {
            words.push(format!("{rest:#x}"));
        }
        if words.is_empty() {
            words.push("none".to_owned());
        }
        Call {
            name,
            fields: vec![("Flags", words.join(" "))],
        }
    }
}
