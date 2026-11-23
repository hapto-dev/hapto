//! `cargo hapto init`: a new plugin crate, from the template in `template/`:
//! a gain in dB and a bypass, its page, a standalone binary for `cargo hapto
//! dev`, its page's parameter map test and a test through the VST3 exporter.
//!
//! The plugin's identity comes from the folder's name, or the flags: its
//! crate, its type, the name hosts show, its vendor, its reverse-DNS id, a
//! fresh VST3 class id, a fresh AU subtype and its vendor's AU code. A host
//! knows a plugin by its ids for good, so a copy of another plugin's would
//! make hosts take one for the other.
//!
//! Hapto's crates are not published yet, so the crate takes them, and the
//! page takes `@hapto/page`, from a checkout by path: the one `--hapto` names,
//! or the one this command was built from.

#[cfg(test)]
mod tests;

use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use crate::bundle;

/// Each file the template writes: where it goes in the new crate, and its
/// text.
pub(crate) const FILES: &[(&str, &str)] = &[
    ("Cargo.toml", include_str!("../template/Cargo.toml.tmpl")),
    ("README.md", include_str!("../template/README.md.tmpl")),
    (".gitignore", include_str!("../template/gitignore.tmpl")),
    ("build.rs", include_str!("../template/build.rs.tmpl")),
    (
        "hapto-juce.cmake",
        include_str!("../template/hapto-juce.cmake.tmpl"),
    ),
    ("src/lib.rs", include_str!("../template/src/lib.rs.tmpl")),
    ("src/page.rs", include_str!("../template/src/page.rs.tmpl")),
    (
        "src/bin/standalone.rs",
        include_str!("../template/src/bin/standalone.rs.tmpl"),
    ),
    (
        "tests/page_params.rs",
        include_str!("../template/tests/page_params.rs.tmpl"),
    ),
    (
        "tests/plugin.rs",
        include_str!("../template/tests/plugin.rs.tmpl"),
    ),
    (
        "tests/juce_settings.rs",
        include_str!("../template/tests/juce_settings.rs.tmpl"),
    ),
    (
        "page/package.json",
        include_str!("../template/page/package.json.tmpl"),
    ),
    (
        "page/index.html",
        include_str!("../template/page/index.html.tmpl"),
    ),
    (
        "page/tsconfig.json",
        include_str!("../template/page/tsconfig.json.tmpl"),
    ),
    (
        "page/vite.config.ts",
        include_str!("../template/page/vite.config.ts.tmpl"),
    ),
    (
        "page/src/main.tsx",
        include_str!("../template/page/src/main.tsx.tmpl"),
    ),
    (
        "page/src/styles.css",
        include_str!("../template/page/src/styles.css.tmpl"),
    ),
    (
        "page/src/generated/params.ts",
        include_str!("../template/page/src/generated/params.ts.tmpl"),
    ),
];

/// Each file the instrument template writes, as [`FILES`] is the effect's:
/// no page, no audio in, and MIDI in.
pub(crate) const INSTRUMENT_FILES: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        include_str!("../template/instrument/Cargo.toml.tmpl"),
    ),
    (
        "README.md",
        include_str!("../template/instrument/README.md.tmpl"),
    ),
    (".gitignore", include_str!("../template/gitignore.tmpl")),
    (
        "hapto-juce.cmake",
        include_str!("../template/instrument/hapto-juce.cmake.tmpl"),
    ),
    (
        "src/lib.rs",
        include_str!("../template/instrument/src/lib.rs.tmpl"),
    ),
    (
        "src/bin/standalone.rs",
        include_str!("../template/instrument/src/bin/standalone.rs.tmpl"),
    ),
    (
        "tests/plugin.rs",
        include_str!("../template/instrument/tests/plugin.rs.tmpl"),
    ),
    (
        "tests/juce_settings.rs",
        include_str!("../template/tests/juce_settings.rs.tmpl"),
    ),
];

/// What the template appends to a plugin's type for the other types it
/// makes: its parameters and its processor.
const MADE: [&str; 3] = ["", "Params", "Processor"];

/// Names none of the types the template makes from a plugin's may take: the
/// template's imports, which a test reads, the names of Rust's prelude it
/// uses, which the type would shadow, and `Self`.
const TAKEN: &[&str] = &[
    "AddSample",
    "Asset",
    "Block",
    "Bool",
    "Box",
    "BusEvent",
    "Changes",
    "Clone",
    "Debug",
    "Editor",
    "Err",
    "Float",
    "Message",
    "None",
    "NoteId",
    "Ok",
    "Option",
    "Page",
    "ParamSet",
    "Params",
    "Parts",
    "Path",
    "Plugin",
    "PluginInfo",
    "ProcessSetup",
    "Processor",
    "Refused",
    "Result",
    "Self",
    "Size",
    "Some",
    "Status",
    "Stereo",
    "StereoOut",
    "TAU",
    "TestHost",
    "UiParams",
    "Voice",
];

/// Rust's keywords, of edition 2024, reserved ones included: a one-word
/// crate's library is named by its word, which the template's code writes as
/// a path.
const KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate",
    "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl",
    "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
    "return", "self", "static", "struct", "super", "trait", "true", "try", "type", "typeof",
    "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

/// Crates a new crate may not be named as: the standard library's, which a
/// library of that name would shadow, and those the template depends on.
const CRATES: &[&str] = &[
    "alloc",
    "core",
    "proc-macro",
    "std",
    "test",
    "hapto-bridge",
    "hapto-host",
    "hapto-params",
    "hapto-standalone",
    "hapto-testkit",
    "hapto-webview",
];

/// What `cargo hapto init` was asked to do.
#[derive(Clone, Debug, PartialEq, Eq, clap::Args)]
pub(crate) struct Options {
    /// The new crate's folder.
    pub(crate) folder: PathBuf,
    /// The name hosts show; from the folder's name by default.
    #[arg(long)]
    pub(crate) name: Option<String>,
    /// The vendor hosts show; "My Company" by default.
    #[arg(long)]
    pub(crate) vendor: Option<String>,
    /// The reverse-DNS id; from the vendor and the crate by default.
    #[arg(long)]
    pub(crate) id: Option<String>,
    /// The Hapto checkout to take the crates from.
    #[arg(long, value_name = "CHECKOUT")]
    pub(crate) hapto: Option<PathBuf>,
    /// An instrument in place of the gain: no audio in, MIDI in, a sine for
    /// each note, and no page.
    #[arg(long)]
    pub(crate) instrument: bool,
}

/// Who the new plugin is, and the names its code uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    /// The package's name: `my-plugin`.
    pub(crate) crate_name: String,
    /// The library's name, as Rust code names it: `my_plugin`.
    pub(crate) lib: String,
    /// The plugin's type: `MyPlugin`.
    pub(crate) type_name: String,
    /// The name hosts show: `My Plugin`.
    pub(crate) name: String,
    /// The vendor hosts show.
    pub(crate) vendor: String,
    /// The reverse-DNS id: `com.mycompany.my-plugin`.
    pub(crate) id: String,
    /// The VST3 class id, a UUID.
    pub(crate) class_id: String,
    /// The AU subtype, four characters.
    pub(crate) subtype: String,
    /// The AU manufacturer's code, four characters, one of them upper case.
    pub(crate) manufacturer: String,
}

impl Identity {
    /// Makes the identity for a crate in a folder named `folder`, with the
    /// flags' `name`, `vendor` and `id` where given, and the 16 bytes
    /// `random` for the class id and the AU subtype.
    ///
    /// # Errors
    ///
    /// Returns which name cannot be used, and why.
    pub(crate) fn new(
        folder: &str,
        name: Option<&str>,
        vendor: Option<&str>,
        id: Option<&str>,
        random: [u8; 16],
    ) -> Result<Identity> {
        let crate_name = crate_name(folder)?;
        let type_name: String = crate_name.split('-').map(capitalized).collect();
        let made = MADE.map(|suffix| format!("{type_name}{suffix}"));
        if let Some(taken) = made.iter().find(|made| TAKEN.contains(&made.as_str())) {
            bail!(
                "a plugin named from {folder:?} would make the type {taken}, which the \
                 template's code uses already: pick another folder name"
            );
        }
        let lib = crate_name.replace('-', "_");
        if bundle::file_name(&lib).is_err() {
            bail!(
                "a plugin named from {folder:?} would build the module {lib}, a device name \
                 Windows keeps, which does not link there: pick another folder name"
            );
        }
        let default_name: Vec<String> = crate_name.split('-').map(capitalized).collect();
        let name = shown(name.unwrap_or(&default_name.join(" ")), "--name")?;
        // The name the bundle takes on disk, refused here as build would.
        bundle::file_name(&name)?;
        let vendor = shown(vendor.unwrap_or("My Company"), "--vendor")?;
        let slug: String = vendor
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase())
            .collect();
        let id = match id {
            Some(id) => reverse_dns(id)?,
            None if slug.is_empty() => {
                bail!("--vendor {vendor:?} has no letter or digit to make the id from: pass --id");
            }
            None => format!("com.{slug}.{crate_name}"),
        };
        Ok(Identity {
            lib,
            subtype: subtype(random),
            manufacturer: au_code(&vendor),
            class_id: class_id(random),
            crate_name,
            type_name,
            name,
            vendor,
            id,
        })
    }
}

/// Returns the package name a folder named `folder` gives: lower case, with
/// `-` between words.
fn crate_name(folder: &str) -> Result<String> {
    let name: String = folder
        .chars()
        .map(|c| match c {
            ' ' | '_' => '-',
            c => c.to_ascii_lowercase(),
        })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let words: Vec<&str> = name.split('-').filter(|w| !w.is_empty()).collect();
    let name = words.join("-");
    if !name.starts_with(|c: char| c.is_ascii_lowercase()) {
        bail!(
            "{folder:?} gives no crate name: it starts with a letter and holds letters, digits \
             and hyphens"
        );
    }
    if KEYWORDS.contains(&name.as_str()) {
        bail!(
            "{folder:?} gives the crate name {name}, a Rust keyword, which its code cannot \
             name its library by: pick another folder name"
        );
    }
    if CRATES.contains(&name.as_str()) {
        bail!(
            "{folder:?} gives the crate name {name}, which a crate it builds with has: pick \
             another folder name"
        );
    }
    Ok(name)
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_ascii_uppercase().to_string() + chars.as_str()
    })
}

/// Checks a name hosts show: letters, digits, spaces and `-_.'`, which every
/// file of the template takes as written.
fn shown(text: &str, flag: &str) -> Result<String> {
    let fine = |c: char| c.is_ascii_alphanumeric() || " -_.'".contains(c);
    if text.trim().is_empty() || !text.chars().all(fine) {
        bail!("{flag} {text:?}: a name of letters, digits, spaces and -_.' only");
    }
    Ok(text.trim().to_owned())
}

/// Checks a reverse-DNS id: lower-case labels of letters, digits and `-`,
/// two or more, joined by dots.
fn reverse_dns(id: &str) -> Result<String> {
    let label = |l: &str| {
        !l.is_empty()
            && l.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    };
    if id.split('.').count() < 2 || !id.split('.').all(label) {
        bail!("--id {id:?}: a reverse-DNS id, com.vendor.product, in lower case");
    }
    Ok(id.to_owned())
}

/// Returns a four-character AU code from `text`: its first letters and
/// digits, padded with `x`, its first letter upper case, since Apple keeps
/// codes of lower case alone for its own.
fn au_code(text: &str) -> String {
    let mut code: Vec<char> = text
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .chain(std::iter::repeat('x'))
        .take(4)
        .collect();
    match code.iter_mut().find(|c| c.is_ascii_alphabetic()) {
        Some(letter) => *letter = letter.to_ascii_uppercase(),
        None => code[0] = 'H',
    }
    code.into_iter().collect()
}

/// Returns an AU subtype drawn from the last four of 16 random bytes, as
/// letters and digits with a capital, as [`au_code`] writes one. A host
/// tells a vendor's plugins apart by it, so it is drawn rather than made
/// from the name, which two plugins can begin alike.
fn subtype(random: [u8; 16]) -> String {
    const ALPHABET: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let drawn: String = random[12..]
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(*byte) % ALPHABET.len()]))
        .collect();
    au_code(&drawn)
}

/// Returns a random UUID, version 4, from 16 random bytes, as VST3 class ids
/// are written: 8-4-4-4-12 hex digits.
pub(crate) fn class_id(mut random: [u8; 16]) -> String {
    random[6] = (random[6] & 0x0f) | 0x40;
    random[8] = (random[8] & 0x3f) | 0x80;
    let hex: String = random.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// Returns `text` with each placeholder of the template filled: the
/// identity's, and `{{hapto}}`, the checkout's path with `/` between its
/// parts.
pub(crate) fn render(text: &str, identity: &Identity, hapto: &str) -> String {
    [
        ("{{crate}}", identity.crate_name.as_str()),
        ("{{lib}}", &identity.lib),
        ("{{type}}", &identity.type_name),
        ("{{name}}", &identity.name),
        ("{{vendor}}", &identity.vendor),
        ("{{id}}", &identity.id),
        ("{{class_id}}", &identity.class_id),
        ("{{subtype}}", &identity.subtype),
        ("{{manufacturer}}", &identity.manufacturer),
        ("{{hapto}}", hapto),
    ]
    .iter()
    .fold(text.to_owned(), |text, (key, value)| {
        text.replace(key, value)
    })
}

/// Runs `cargo hapto init` with `options`.
///
/// # Errors
///
/// Returns why it wrote nothing: a folder that is not empty, a name it
/// cannot use, no checkout.
pub(crate) fn run(options: &Options) -> Result<()> {
    let folder_name = folder_name(&options.folder)?;
    let mut random = [0_u8; 16];
    getrandom::fill(&mut random).context("random bytes for the class id")?;
    let identity = Identity::new(
        &folder_name,
        options.name.as_deref(),
        options.vendor.as_deref(),
        options.id.as_deref(),
        random,
    )?;
    let hapto = checkout(options.hapto.as_deref())?;
    let files = if options.instrument {
        INSTRUMENT_FILES
    } else {
        FILES
    };
    write(&options.folder, &identity, &hapto, files)?;
    println!(
        "wrote {} ({}, {}) in {}\n\nnext:\n{}  cargo hapto dev",
        identity.name,
        identity.crate_name,
        identity.id,
        options.folder.display(),
        cd_line(&options.folder),
    );
    Ok(())
}

/// Returns the line of the next steps that changes to `folder`, quoted when
/// it holds a space so it can be pasted as it is; none when it is the folder
/// the command ran in.
pub(crate) fn cd_line(folder: &Path) -> String {
    if folder.components().all(|part| part == Component::CurDir) {
        return String::new();
    }
    let shown = folder.display().to_string();
    if shown.contains(' ') {
        format!("  cd \"{shown}\"\n")
    } else {
        format!("  cd {shown}\n")
    }
}

/// Returns the name of the folder `folder` names, which the crate takes its
/// names from. A path that ends in `.` or `..`, as `init .` after `git init`
/// gives, names a folder that exists, read where it is.
///
/// # Errors
///
/// Says it names no folder: the root, or a `.` that cannot be read.
fn folder_name(folder: &Path) -> Result<String> {
    let named = match folder.file_name() {
        Some(_) => folder.to_path_buf(),
        None => folder
            .canonicalize()
            .with_context(|| folder.display().to_string())?,
    };
    named
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .with_context(|| format!("{} names no folder", folder.display()))
}

/// Returns the Hapto checkout to take the crates from, as the template
/// writes its path: `checkout`, else the one this command was built from.
///
/// # Errors
///
/// Returns why it is no checkout.
pub(crate) fn checkout(checkout: Option<&Path>) -> Result<String> {
    let built_from = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = checkout.unwrap_or(&built_from);
    let dir = std::path::absolute(dir).with_context(|| dir.display().to_string())?;
    let is_checkout =
        dir.join("crates/host/Cargo.toml").is_file() && dir.join("page/package.json").is_file();
    if !is_checkout {
        bail!(
            "{} is no Hapto checkout: name one with --hapto, since Hapto's crates are not \
             published yet",
            dir.display()
        );
    }
    Ok(dir.to_string_lossy().replace('\\', "/"))
}

/// Writes the template into `folder`, which must be new or empty.
///
/// # Errors
///
/// Returns why it did not: a folder with something in it, or a write that
/// failed.
fn write(folder: &Path, identity: &Identity, hapto: &str, files: &[(&str, &str)]) -> Result<()> {
    // A folder with only `.git` in it is empty still, as after `git init`.
    let has_files = fs::read_dir(folder)
        .is_ok_and(|mut entries| entries.any(|e| e.is_ok_and(|e| e.file_name() != ".git")));
    if has_files {
        bail!(
            "{} is not empty: init writes a new crate only",
            folder.display()
        );
    }
    for (path, text) in files {
        let file = folder.join(path);
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent).with_context(|| parent.display().to_string())?;
        }
        fs::write(&file, render(text, identity, hapto))
            .with_context(|| file.display().to_string())?;
    }
    Ok(())
}
