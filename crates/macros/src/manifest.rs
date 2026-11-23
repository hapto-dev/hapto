//! `manifest_info!`, which writes a plugin's `hapto_host::PluginInfo`: who the
//! plugin is from `[package.metadata.hapto]` in the plugin crate's manifest,
//! its version and description from `[package]`, and the rest from the fields
//! the macro is given.
//!
//! Cargo reads no `[package.metadata]` table, so the macro checks this one: a
//! key it does not take, a value of the wrong type or form, and a field
//! written both in the manifest and in Rust are refused while compiling, each
//! mistake with its line in the manifest.

use std::ops::Range;
use std::path::Path;

use proc_macro2::{Literal, Span, TokenStream as Tokens};
use quote::{format_ident, quote};
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{Error, FieldValue, Member, Token};
use toml::Spanned;
use toml::de::{DeString, DeTable, DeValue};

/// The keys of `[package.metadata.hapto]`, each a `PluginInfo` field in kebab
/// case.
const KEYS: &[&str] = &[
    "name",
    "vendor",
    "url",
    "email",
    "id",
    "vst3-class-id",
    "au-codes",
    "category",
    "features",
];

/// The `PluginInfo` fields Cargo's own `[package]` gives.
const PACKAGE: &[&str] = &["version", "description"];

/// The keys of `au-codes`, each an `AuCodes` field.
const AU_CODES: &[&str] = &["subtype", "manufacturer"];

/// The categories, each a `hapto_host::Category` variant in kebab case.
const CATEGORIES: &[&str] = &["effect", "instrument"];

/// The features, each a `hapto_host::Feature` variant in kebab case.
const FEATURES: &[&str] = &[
    "analyzer",
    "delay",
    "distortion",
    "dynamics",
    "equalizer",
    "filter",
    "mastering",
    "modulation",
    "pitch-shift",
    "restoration",
    "reverb",
    "spatial",
    "tools",
    "drum",
    "piano",
    "sampler",
    "synthesizer",
];

/// The `PluginInfo` that `input`, the fields written in Rust, completes.
pub(crate) fn expand(input: Tokens) -> syn::Result<Tokens> {
    let fields = Punctuated::<FieldValue, Token![,]>::parse_terminated.parse2(input)?;
    written_once(&fields)?;
    let Some(dir) = std::env::var_os("CARGO_MANIFEST_DIR") else {
        return Err(Error::new(
            Span::call_site(),
            "manifest_info! reads the manifest that cargo names in CARGO_MANIFEST_DIR: build with cargo",
        ));
    };
    let path = Path::new(&dir).join("Cargo.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| Error::new(Span::call_site(), format!("{}: {error}", path.display())))?;
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    match identity(&text, &version) {
        Ok(identity) => Ok(info(&identity, &path, &fields)),
        Err(mistakes) => Err(refusal(&path, mistakes)),
    }
}

/// `layouts: &[],` when the fields leave `layouts` out: a processor in a
/// typed layout runs at that layout's own width, so only a processor in
/// `AnyLayout` lists any.
fn default_layouts(fields: &Punctuated<FieldValue, Token![,]>) -> Option<Tokens> {
    let given = fields
        .iter()
        .any(|field| matches!(&field.member, Member::Named(name) if name == "layouts"));
    (!given).then(|| quote! { layouts: &[], })
}

/// Refuses a field written in Rust that the manifest gives, so each is
/// written in one place.
fn written_once(fields: &Punctuated<FieldValue, Token![,]>) -> syn::Result<()> {
    let mut errors: Option<Error> = None;
    for field in fields {
        let Member::Named(name) = &field.member else {
            continue;
        };
        let text = name.to_string();
        let key = text.replace('_', "-");
        let message = if PACKAGE.contains(&text.as_str()) {
            format!("`{name}` is the package's own: set it in the manifest's [package]")
        } else if KEYS.contains(&key.as_str()) {
            format!("`{name}` comes from the manifest: set `{key}` in its [package.metadata.hapto]")
        } else {
            continue;
        };
        let error = Error::new_spanned(name, message);
        match &mut errors {
            Some(errors) => errors.combine(error),
            None => errors = Some(error),
        }
    }
    errors.map_or(Ok(()), Err)
}

/// One error at the macro's call that lists every mistake, each with the
/// manifest's path and the line, as an editor links them.
fn refusal(path: &Path, mistakes: Vec<Mistake>) -> Error {
    let mut errors = mistakes.into_iter().map(|mistake| {
        let place = match mistake.line {
            Some(line) => format!("{}:{line}", path.display()),
            None => path.display().to_string(),
        };
        Error::new(Span::call_site(), format!("{place}: {}", mistake.message))
    });
    let mut first = errors
        .next()
        .unwrap_or_else(|| Error::new(Span::call_site(), "the manifest was refused"));
    first.extend(errors);
    first
}

/// The `PluginInfo` expression: the identity, then the fields written in
/// Rust.
fn info(
    identity: &Identity,
    manifest: &Path,
    fields: &Punctuated<FieldValue, Token![,]>,
) -> Tokens {
    let Identity {
        name,
        vendor,
        url,
        email,
        id,
        ..
    } = identity;
    let manifest = Literal::string(&manifest.to_string_lossy());
    let class_id = identity
        .vst3_class_id
        .iter()
        .map(|&byte| Literal::u8_suffixed(byte));
    let subtype = Literal::byte_string(&identity.subtype);
    let manufacturer = Literal::byte_string(&identity.manufacturer);
    let category = format_ident!("{}", identity.category);
    let features = identity
        .features
        .iter()
        .map(|feature| format_ident!("{}", feature));
    let layouts = default_layouts(fields);
    quote! {
        {
            // Cargo builds the crate again when a file it includes changes.
            const _: &str = ::core::include_str!(#manifest);
            ::hapto_host::PluginInfo {
                name: #name,
                vendor: #vendor,
                url: #url,
                email: #email,
                description: ::core::env!("CARGO_PKG_DESCRIPTION"),
                version: ::core::env!("CARGO_PKG_VERSION"),
                id: #id,
                vst3_class_id: [#(#class_id),*],
                au_codes: ::hapto_host::AuCodes {
                    subtype: *#subtype,
                    manufacturer: *#manufacturer,
                },
                category: ::hapto_host::Category::#category,
                features: &[#(::hapto_host::Feature::#features),*],
                #layouts
                #fields
            }
        }
    }
}

/// Who a plugin is, as its manifest says.
#[derive(Debug, Default, PartialEq)]
struct Identity {
    name: String,
    vendor: String,
    url: String,
    email: String,
    id: String,
    vst3_class_id: [u8; 16],
    subtype: [u8; 4],
    manufacturer: [u8; 4],
    /// The `Category` variant.
    category: String,
    /// The `Feature` variants, in the manifest's order.
    features: Vec<String>,
}

/// What is wrong in the manifest, and the line it is on, when it is on one.
#[derive(Debug, PartialEq)]
struct Mistake {
    line: Option<usize>,
    message: String,
}

/// Reads the identity from `manifest`, the text of a whole `Cargo.toml`, and
/// checks `version`, the package's own; or returns every mistake in either.
fn identity(manifest: &str, version: &str) -> Result<Identity, Vec<Mistake>> {
    let document = DeTable::parse(manifest).map_err(|error| {
        vec![Mistake {
            line: None,
            message: error.to_string(),
        }]
    })?;
    let mut reader = Reader {
        text: manifest,
        mistakes: Vec::new(),
    };
    if !is_plain_version(version) {
        reader.refuse(
            None,
            format!("the package's version `{version}` is not major.minor.patch, as a plugin's is"),
        );
    }
    let Some(hapto) = reader.hapto(document.get_ref()) else {
        return Err(reader.mistakes);
    };
    reader.unknown(hapto, KEYS, "a key of [package.metadata.hapto]");
    let (subtype, manufacturer) = reader.au_codes(hapto);
    let identity = Identity {
        name: reader.required(hapto, "name", "the name hosts list").1,
        vendor: reader.required(hapto, "vendor", "who makes it").1,
        url: reader.optional(hapto, "url"),
        email: reader.optional(hapto, "email"),
        id: reader
            .required(hapto, "id", "a reverse-DNS id, com.vendor.product")
            .1,
        vst3_class_id: reader.class_id(hapto),
        subtype,
        manufacturer,
        category: reader.category(hapto),
        features: reader.features(hapto),
    };
    if reader.mistakes.is_empty() {
        Ok(identity)
    } else {
        Err(reader.mistakes)
    }
}

/// A table, the line its key is on, and how a message names it.
#[derive(Clone, Copy)]
struct Table<'t, 'i> {
    table: &'t DeTable<'i>,
    line: Option<usize>,
    name: &'static str,
}

/// Reads the manifest's text, keeping each mistake it finds.
struct Reader<'a> {
    text: &'a str,
    mistakes: Vec<Mistake>,
}

impl Reader<'_> {
    /// The line, from 1, that `span` starts on.
    fn line(&self, span: Range<usize>) -> Option<usize> {
        let before = self.text.get(..span.start)?;
        Some(before.matches('\n').count() + 1)
    }

    fn refuse(&mut self, line: Option<usize>, message: String) {
        self.mistakes.push(Mistake { line, message });
    }

    /// `[package.metadata.hapto]`, or `None` with the mistake kept.
    fn hapto<'t, 'i>(&mut self, document: &'t DeTable<'i>) -> Option<Table<'t, 'i>> {
        let mut table = Table {
            table: document,
            line: None,
            name: "the manifest",
        };
        for key in ["package", "metadata", "hapto"] {
            let Some((name, value)) = entry(table.table, key) else {
                self.refuse(
                    None,
                    "the manifest has no [package.metadata.hapto], where manifest_info! reads who the plugin is".to_owned(),
                );
                return None;
            };
            let line = self.line(name.span());
            let DeValue::Table(inner) = value.get_ref() else {
                self.refuse(
                    line,
                    format!("`{key}` is a table, not {}", kind(value.get_ref())),
                );
                return None;
            };
            table = Table {
                table: inner,
                line,
                name: "[package.metadata.hapto]",
            };
        }
        Some(table)
    }

    /// Refuses each key of `table` that is not one of `keys`, listing them.
    fn unknown(&mut self, table: Table<'_, '_>, keys: &[&str], what: &str) {
        for (key, _) in table.table.iter() {
            let text: &str = key.get_ref();
            if keys.contains(&text) {
                continue;
            }
            let message = if PACKAGE.contains(&text) {
                format!("`{text}` is the package's own: set it in [package]")
            } else {
                format!("`{text}` is not {what}: they are {}", list(keys))
            };
            let line = self.line(key.span());
            self.refuse(line, message);
        }
    }

    /// The string under `key`, and the line it is on; refused when it is
    /// missing or empty, with `what` saying what it is.
    fn required(&mut self, table: Table<'_, '_>, key: &str, what: &str) -> (Option<usize>, String) {
        let Some((name, value)) = entry(table.table, key) else {
            let message = format!("{} has no `{key}`: {what}", table.name);
            self.refuse(table.line, message);
            return (table.line, String::new());
        };
        let line = self.line(name.span());
        let Some(text) = self.string(key, line, value) else {
            return (line, String::new());
        };
        if text.is_empty() {
            self.refuse(line, format!("`{key}` is empty: {what}"));
        }
        (line, text)
    }

    /// The string under `key`, empty when it is missing.
    fn optional(&mut self, table: Table<'_, '_>, key: &str) -> String {
        let Some((name, value)) = entry(table.table, key) else {
            return String::new();
        };
        let line = self.line(name.span());
        self.string(key, line, value).unwrap_or_default()
    }

    /// `value` as a string, or `None` with the mistake kept.
    fn string(
        &mut self,
        key: &str,
        line: Option<usize>,
        value: &Spanned<DeValue<'_>>,
    ) -> Option<String> {
        if let DeValue::String(text) = value.get_ref() {
            return Some(text.to_string());
        }
        self.refuse(
            line,
            format!("`{key}` is a string, not {}", kind(value.get_ref())),
        );
        None
    }

    /// The VST3 class id, written as a UUID.
    fn class_id(&mut self, table: Table<'_, '_>) -> [u8; 16] {
        let what = "a UUID made once for this plugin, such as uuidgen prints";
        let (line, text) = self.required(table, "vst3-class-id", what);
        if text.is_empty() {
            return [0; 16];
        }
        uuid(&text).unwrap_or_else(|| {
            let message =
                format!("`vst3-class-id` is a UUID, 8-4-4-4-12 hex digits: \"{text}\" is not");
            self.refuse(line, message);
            [0; 16]
        })
    }

    /// The AU codes: the subtype and the manufacturer's code.
    fn au_codes(&mut self, table: Table<'_, '_>) -> ([u8; 4], [u8; 4]) {
        let example = "{ subtype = \"Tone\", manufacturer = \"Exmp\" }";
        let Some((name, value)) = entry(table.table, "au-codes") else {
            let message = format!("{} has no `au-codes`: {example}", table.name);
            self.refuse(table.line, message);
            return ([0; 4], [0; 4]);
        };
        let line = self.line(name.span());
        let DeValue::Table(codes) = value.get_ref() else {
            let message = format!(
                "`au-codes` is a table, {example}, not {}",
                kind(value.get_ref())
            );
            self.refuse(line, message);
            return ([0; 4], [0; 4]);
        };
        let codes = Table {
            table: codes,
            line,
            name: "`au-codes`",
        };
        self.unknown(codes, AU_CODES, "an AU code");
        (
            self.code(codes, "subtype"),
            self.code(codes, "manufacturer"),
        )
    }

    /// One AU code: four printable ASCII characters.
    fn code(&mut self, codes: Table<'_, '_>, key: &str) -> [u8; 4] {
        let what = "four printable ASCII characters";
        let (line, text) = self.required(codes, key, what);
        if text.is_empty() {
            return [0; 4];
        }
        let printable = text.bytes().all(|byte| matches!(byte, b' '..=b'~'));
        match <[u8; 4]>::try_from(text.as_bytes()) {
            Ok(code) if printable => code,
            _ => {
                self.refuse(line, format!("`{key}` is {what}: \"{text}\" is not"));
                [0; 4]
            }
        }
    }

    /// The category's `Category` variant.
    fn category(&mut self, table: Table<'_, '_>) -> String {
        let what = format!("{} or {}", CATEGORIES[0], CATEGORIES[1]);
        let (line, text) = self.required(table, "category", &what);
        if CATEGORIES.contains(&text.as_str()) {
            return variant(&text);
        }
        if !text.is_empty() {
            self.refuse(line, format!("`{text}` is not a category: one is {what}"));
        }
        String::new()
    }

    /// The features' `Feature` variants, none when the key is missing.
    fn features(&mut self, table: Table<'_, '_>) -> Vec<String> {
        let Some((name, value)) = entry(table.table, "features") else {
            return Vec::new();
        };
        let DeValue::Array(items) = value.get_ref() else {
            let line = self.line(name.span());
            let message = format!(
                "`features` is an array of strings, not {}",
                kind(value.get_ref())
            );
            self.refuse(line, message);
            return Vec::new();
        };
        let mut features = Vec::new();
        for item in items.iter() {
            let line = self.line(item.span());
            let message = match item.get_ref() {
                DeValue::String(text) if FEATURES.contains(&&**text) => {
                    let feature = variant(text);
                    if !features.contains(&feature) {
                        features.push(feature);
                        continue;
                    }
                    format!("the feature `{text}` is listed twice")
                }
                DeValue::String(text) => {
                    format!("`{text}` is not a feature: they are {}", list(FEATURES))
                }
                other => format!("`features` holds strings, not {}", kind(other)),
            };
            self.refuse(line, message);
        }
        features
    }
}

/// The key and value under `key` in `table`.
fn entry<'t, 'i>(
    table: &'t DeTable<'i>,
    key: &str,
) -> Option<(&'t Spanned<DeString<'i>>, &'t Spanned<DeValue<'i>>)> {
    table.iter().find(|(name, _)| name.get_ref() == key)
}

/// What kind of value `value` is, with its article.
fn kind(value: &DeValue<'_>) -> &'static str {
    match value {
        DeValue::String(_) => "a string",
        DeValue::Integer(_) => "an integer",
        DeValue::Float(_) => "a float",
        DeValue::Boolean(_) => "a boolean",
        DeValue::Datetime(_) => "a date or time",
        DeValue::Array(_) => "an array",
        DeValue::Table(_) => "a table",
    }
}

/// `names` as a sentence lists them: "a, b and c".
fn list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

/// The variant a kebab-case name stands for: `pitch-shift` is `PitchShift`.
fn variant(name: &str) -> String {
    name.split('-')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect()
}

/// Whether `version` is three numbers, `major.minor.patch`, with no
/// pre-release or build part.
fn is_plain_version(version: &str) -> bool {
    let parts: Vec<&str> = version.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

/// The 16 bytes of a UUID written 8-4-4-4-12 in hex digits.
fn uuid(text: &str) -> Option<[u8; 16]> {
    let groups: Vec<&str> = text.split('-').collect();
    if !groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12]) {
        return None;
    }
    let digits = groups.concat();
    if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = [0; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let pair = digits.get(index * 2..index * 2 + 2)?;
        *byte = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use syn::{Item, ItemEnum};

    use super::*;

    /// The gain's manifest, as the example's is.
    const GAIN: &str = r#"[package]
name = "hapto-example-gain"
description = "A gain with three modes, a label and MIDI learn"
version.workspace = true

[package.metadata.hapto]
name = "Hapto Example Gain"
vendor = "Hapto"
id = "dev.hapto.example-gain"
vst3-class-id = "5b0ad271-3e96-4c18-a37f-e0248db56a19"
au-codes = { subtype = "SmGn", manufacturer = "Hapt" }
category = "effect"
features = ["equalizer", "pitch-shift"]
"#;

    /// The messages of the mistakes in `manifest`, each after its line.
    fn mistakes(manifest: &str, version: &str) -> Vec<String> {
        let mistakes = identity(manifest, version).expect_err("the manifest is refused");
        mistakes
            .into_iter()
            .map(|mistake| match mistake.line {
                Some(line) => format!("{line}: {}", mistake.message),
                None => mistake.message,
            })
            .collect()
    }

    /// The variants of `hapto_host`'s enum `name`, in their order.
    fn host_variants(name: &str) -> Vec<String> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../host/src/info.rs");
        let source = std::fs::read_to_string(&path).expect("hapto-host's info.rs");
        let file = syn::parse_file(&source).expect("info.rs parses");
        let found = file.items.into_iter().find_map(|item| match item {
            Item::Enum(ItemEnum {
                ident, variants, ..
            }) if ident == name => Some(variants),
            _ => None,
        });
        let variants = found.unwrap_or_else(|| panic!("info.rs has no enum {name}"));
        variants
            .iter()
            .map(|variant| variant.ident.to_string())
            .collect()
    }

    #[test]
    fn the_categories_and_features_name_the_hosts_variants() {
        let named = |names: &[&str]| names.iter().map(|name| variant(name)).collect::<Vec<_>>();
        assert_eq!(named(CATEGORIES), host_variants("Category"));
        assert_eq!(named(FEATURES), host_variants("Feature"));
    }

    #[test]
    fn a_manifest_reads_as_the_plugins_identity() {
        let identity = identity(GAIN, "0.1.0").expect("the manifest is read");
        let expected = Identity {
            name: "Hapto Example Gain".to_owned(),
            vendor: "Hapto".to_owned(),
            id: "dev.hapto.example-gain".to_owned(),
            vst3_class_id: 0x5b0a_d271_3e96_4c18_a37f_e024_8db5_6a19_u128.to_be_bytes(),
            subtype: *b"SmGn",
            manufacturer: *b"Hapt",
            category: "Effect".to_owned(),
            features: vec!["Equalizer".to_owned(), "PitchShift".to_owned()],
            ..Identity::default()
        };
        assert_eq!(identity, expected);
    }

    #[test]
    fn every_mistake_is_refused_on_its_line() {
        let manifest = r#"[package]
name = "hapto-example-gain"

[package.metadata.hapto]
name = ""
vendr = "Hapto"
version = "1.0.0"
id = 7
vst3-class-id = "5b0ad271-3e96-4c18-a37f"
au-codes = { subtype = "SmG", manufacturer = "Hapt", type = "aufx" }
category = "efect"
features = ["equaliser", "delay", "delay"]
"#;
        let expected = [
            "6: `vendr` is not a key of [package.metadata.hapto]: they are name, vendor, url, email, id, vst3-class-id, au-codes, category and features",
            "7: `version` is the package's own: set it in [package]",
            "10: `type` is not an AU code: they are subtype and manufacturer",
            "10: `subtype` is four printable ASCII characters: \"SmG\" is not",
            "5: `name` is empty: the name hosts list",
            "4: [package.metadata.hapto] has no `vendor`: who makes it",
            "8: `id` is a string, not an integer",
            "9: `vst3-class-id` is a UUID, 8-4-4-4-12 hex digits: \"5b0ad271-3e96-4c18-a37f\" is not",
            "11: `efect` is not a category: one is effect or instrument",
            "12: `equaliser` is not a feature: they are analyzer, delay, distortion, dynamics, equalizer, filter, mastering, modulation, pitch-shift, restoration, reverb, spatial, tools, drum, piano, sampler and synthesizer",
            "12: the feature `delay` is listed twice",
        ];
        assert_eq!(mistakes(manifest, "0.1.0"), expected);
    }

    #[test]
    fn a_manifest_without_the_table_is_refused() {
        let manifest = "[package]\nname = \"plugin\"\n";
        assert_eq!(
            mistakes(manifest, "0.1.0"),
            [
                "the manifest has no [package.metadata.hapto], where manifest_info! reads who the plugin is"
            ]
        );
    }

    #[test]
    fn a_version_with_a_pre_release_part_is_refused() {
        assert_eq!(
            mistakes(GAIN, "0.2.0-beta.1"),
            ["the package's version `0.2.0-beta.1` is not major.minor.patch, as a plugin's is"]
        );
        assert!(is_plain_version("10.0.12"));
        for version in ["1.0", "1.0.0.0", "1..0", "1.0.x", "1.0.0+build"] {
            assert!(!is_plain_version(version), "{version}");
        }
    }

    #[test]
    fn a_field_the_manifest_gives_is_refused_in_rust() {
        let fields: Punctuated<FieldValue, Token![,]> = syn::parse_quote! {
            layouts: &[Width::STEREO],
            name: "Gain",
            vst3_class_id: [0; 16],
            version: "1.0.0",
        };
        let messages: Vec<String> = written_once(&fields)
            .expect_err("the fields are refused")
            .into_iter()
            .map(|error| error.to_string())
            .collect();
        assert_eq!(
            messages,
            [
                "`name` comes from the manifest: set `name` in its [package.metadata.hapto]",
                "`vst3_class_id` comes from the manifest: set `vst3-class-id` in its [package.metadata.hapto]",
                "`version` is the package's own: set it in the manifest's [package]",
            ]
        );
    }

    #[test]
    fn layouts_left_out_are_empty() {
        let typed: Punctuated<FieldValue, Token![,]> = syn::parse_quote! {
            midi_input: false,
            midi_output: false,
        };
        let listed: Punctuated<FieldValue, Token![,]> = syn::parse_quote! {
            layouts: &[Width::STEREO, Width::MONO],
            midi_input: false,
        };
        assert_eq!(
            default_layouts(&typed).map(|tokens| tokens.to_string()),
            Some(quote! { layouts: &[], }.to_string())
        );
        assert!(default_layouts(&listed).is_none());
    }

    #[test]
    fn a_uuid_reads_as_its_bytes() {
        assert_eq!(
            uuid("5B0AD271-3e96-4c18-a37f-e0248db56a19"),
            Some(0x5b0a_d271_3e96_4c18_a37f_e024_8db5_6a19_u128.to_be_bytes())
        );
        for text in [
            "5b0ad2713e964c18a37fe0248db56a19",
            "5b0ad271-3e96-4c18-a37f-e0248db56a1",
            "5b0ad271-3e96-4c18-a37f-e0248db56a1g",
            "+b0ad271-3e96-4c18-a37f-e0248db56a19",
        ] {
            assert_eq!(uuid(text), None, "{text}");
        }
    }
}
