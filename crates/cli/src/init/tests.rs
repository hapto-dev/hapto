//! `cargo hapto init` makes a plugin's identity from its folder's name and
//! its flags, writes fresh ids, fills every placeholder of the template, and
//! writes only into a new or empty folder.

use std::path::{Path, PathBuf};

use super::{
    FILES, INSTRUMENT_FILES, Identity, Options, TAKEN, au_code, cd_line, class_id, crate_name,
    folder_name, render, write,
};

fn identity(folder: &str) -> Identity {
    Identity::new(folder, None, None, None, [7; 16]).expect("an identity")
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join("cargo-hapto-tests")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn the_folder_and_flags_are_read() {
    let parse = crate::parse_as::<Options>;
    let options =
        parse(&["my-plugin", "--name", "My Plugin", "--hapto", "../hapto"]).expect("valid");
    assert_eq!(options.folder, Path::new("my-plugin"));
    assert_eq!(options.name.as_deref(), Some("My Plugin"));
    assert_eq!(options.hapto, Some(PathBuf::from("../hapto")));
    assert!(parse(&[]).is_err(), "no folder");
    assert!(parse(&["a", "b"]).is_err(), "two folders");
    assert!(parse(&["a", "--name"]).is_err(), "no name");
}

#[test]
fn a_folders_name_gives_the_crate_its_type_and_its_names() {
    let made = identity("My_Plugin 2");
    assert_eq!(made.crate_name, "my-plugin-2");
    assert_eq!(made.lib, "my_plugin_2");
    assert_eq!(made.type_name, "MyPlugin2");
    assert_eq!(made.name, "My Plugin 2");
    assert_eq!(made.vendor, "My Company");
    assert_eq!(made.id, "com.mycompany.my-plugin-2");
    assert_eq!(
        (made.subtype.as_str(), made.manufacturer.as_str()),
        ("Hhhh", "Myco"),
        "the subtype from the random bytes, each 7, the manufacturer from the vendor"
    );
}

#[test]
fn the_flags_name_the_plugin_its_vendor_and_its_id() {
    let made = Identity::new(
        "tone",
        Some("Tone Shaper"),
        Some("Acme Audio"),
        Some("io.acme.tone"),
        [0; 16],
    )
    .expect("an identity");
    assert_eq!(
        (made.name.as_str(), made.vendor.as_str()),
        ("Tone Shaper", "Acme Audio")
    );
    assert_eq!(made.id, "io.acme.tone");
    assert_eq!(made.manufacturer, "Acme");
}

#[test]
fn names_the_template_cannot_take_are_refused() {
    for folder in ["2fast", "---", "", "plugin", "self"] {
        assert!(
            Identity::new(folder, None, None, None, [0; 16]).is_err(),
            "{folder:?}"
        );
    }
    let quoted = Identity::new("tone", Some("Tone \"Pro\""), None, None, [0; 16]);
    assert!(quoted.is_err(), "a quote would end a string");
    for id in ["tone", "com..tone", "Com.Acme.Tone", "com.acme.tone!"] {
        assert!(
            Identity::new("tone", None, None, Some(id), [0; 16]).is_err(),
            "{id}"
        );
    }
    assert_eq!(crate_name("Über Gain").unwrap(), "ber-gain");
}

#[test]
fn names_the_generated_code_could_not_compile_are_refused() {
    // A keyword as the library's name, a type that shadows the prelude's,
    // a crate the standard library or the template depends on, and a type
    // whose own name or parameters' name a template file imports.
    for folder in [
        "ui",
        "refused",
        "test-host",
        "loop",
        "type",
        "gen",
        "box",
        "static",
        "option",
        "box-",
        "std",
        "core",
        "hapto-host",
    ] {
        assert!(
            Identity::new(folder, None, None, None, [0; 16]).is_err(),
            "{folder:?}"
        );
    }
    for folder in ["my-loop", "type-one", "gain"] {
        assert!(
            Identity::new(folder, None, None, None, [0; 16]).is_ok(),
            "{folder:?}"
        );
    }
}

/// The types a Rust file of the template imports, the plugin's own left out.
fn imported(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut statement: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if statement.is_none() && (line.starts_with("use ") || line.starts_with("pub use ")) {
            statement = Some(String::new());
        }
        let Some(open) = statement.as_mut() else {
            continue;
        };
        open.push_str(line);
        if !line.ends_with(';') {
            continue;
        }
        let done = statement.take().unwrap_or_default();
        if done.contains("{{") {
            continue;
        }
        for piece in done.trim_end_matches(';').split([',', '{', '}']) {
            let name = piece.trim().rsplit("::").next().unwrap_or_default();
            // A type, not a constant, which no type's name can clash with.
            let a_type = name.starts_with(|c: char| c.is_ascii_uppercase())
                && name.contains(|c: char| c.is_ascii_lowercase());
            if a_type {
                names.push(name.to_owned());
            }
        }
    }
    names
}

#[test]
fn every_type_a_template_file_imports_is_taken() {
    let mut read = 0;
    let files = FILES.iter().chain(INSTRUMENT_FILES);
    for (path, text) in files.filter(|(path, _)| path.ends_with(".rs")) {
        for name in imported(text) {
            assert!(TAKEN.contains(&name.as_str()), "{path} imports {name}");
            read += 1;
        }
    }
    assert!(read > 20, "the imports were read: {read}");
}

#[test]
fn the_au_subtype_is_drawn_at_random_and_the_manufacturer_is_the_vendor_s() {
    let one = Identity::new("compressor", None, None, None, [1; 16]).expect("an identity");
    let two = Identity::new("compander", None, None, None, [2; 16]).expect("an identity");
    assert_ne!(one.subtype, two.subtype, "two plugins of one vendor");
    assert_eq!(one.manufacturer, two.manufacturer, "one vendor's code");
    for made in [&one, &two] {
        let code = &made.subtype;
        assert_eq!(code.len(), 4, "{code}");
        assert!(code.chars().all(|c| c.is_ascii_alphanumeric()), "{code}");
        assert!(code.chars().any(|c| c.is_ascii_uppercase()), "{code}");
    }
}

#[test]
fn each_init_writes_a_fresh_version_4_class_id() {
    let id = class_id([0xff; 16]);
    assert_eq!(id, "ffffffff-ffff-4fff-bfff-ffffffffffff");
    assert_eq!(class_id([0; 16]), "00000000-0000-4000-8000-000000000000");
    let lengths: Vec<usize> = id.split('-').map(str::len).collect();
    assert_eq!(lengths, [8, 4, 4, 4, 12]);
    assert_ne!(class_id([1; 16]), class_id([2; 16]));
}

#[test]
fn an_au_code_is_four_characters_with_a_capital() {
    assert_eq!(au_code("Gain"), "Gain");
    assert_eq!(au_code("ab"), "Abxx");
    assert_eq!(au_code("3D Labs"), "3Dla");
    assert_eq!(au_code("1234 5"), "H234");
}

#[test]
fn every_placeholder_of_every_file_is_filled() {
    let made = identity("my-plugin");
    for (path, text) in FILES.iter().chain(INSTRUMENT_FILES) {
        let rendered = render(text, &made, "C:/src/hapto");
        assert!(!rendered.contains("{{"), "{path} keeps a placeholder");
    }
    let manifest = render(FILES[0].1, &made, "C:/src/hapto");
    assert!(manifest.contains("name = \"my-plugin\""));
    assert!(manifest.contains(&format!("vst3-class-id = \"{}\"", made.class_id)));
    assert!(manifest.contains("path = \"C:/src/hapto/crates/host\""));
}

#[test]
fn a_new_crate_is_written_only_into_a_new_or_empty_folder() {
    let dir = temp("write");
    let made = identity("write");
    write(&dir, &made, "C:/src/hapto", FILES).expect("a new folder");
    for (path, _) in FILES {
        assert!(dir.join(path).is_file(), "{path}");
    }
    assert!(
        write(&dir, &made, "C:/src/hapto", FILES).is_err(),
        "not empty now"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_folder_git_alone_holds_takes_the_new_crate() {
    // As after `git init`, then `cargo hapto init .`.
    let dir = temp("git-only");
    std::fs::create_dir_all(dir.join(".git")).expect("made");
    write(&dir, &identity("git-only"), "C:/src/hapto", FILES).expect("only .git");
    assert!(dir.join("Cargo.toml").is_file());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_folder_s_name_is_read_from_a_dot_or_a_dot_dot_too() {
    // Tests run in the crate's own folder, `crates/cli`.
    assert_eq!(folder_name(Path::new(".")).unwrap(), "cli");
    assert_eq!(folder_name(Path::new("./")).unwrap(), "cli");
    assert_eq!(folder_name(Path::new("..")).unwrap(), "crates");
    assert_eq!(
        folder_name(Path::new("new/my-plugin")).unwrap(),
        "my-plugin",
        "a folder not made yet"
    );
}

#[test]
fn init_refuses_a_name_the_bundle_or_the_module_cannot_take() {
    // A crate whose module would be a device's name does not link on
    // Windows, and a name that leaves no file cannot name the bundle.
    for folder in ["con", "com1", "aux"] {
        let refused = Identity::new(folder, None, None, None, [0; 16]).expect_err(folder);
        assert!(
            format!("{refused:#}").contains("device"),
            "{folder}: {refused}"
        );
    }
    let refused = Identity::new("tone", Some("..."), None, None, [0; 16]);
    assert!(refused.is_err(), "no file name is left of it");
    let refused = Identity::new("tone", Some("Aux.Send"), None, None, [0; 16]);
    assert!(refused.is_err(), "the bundle would be the AUX device");
}

#[test]
fn a_vendor_with_no_letters_cannot_make_the_default_id() {
    let refused = Identity::new("tone", None, Some("_"), None, [0; 16]).expect_err("com..tone");
    assert!(
        format!("{refused:#}").contains("--id"),
        "says to give one: {refused}"
    );
    let given = Identity::new("tone", None, Some("_"), Some("io.acme.tone"), [0; 16]);
    assert!(given.is_ok(), "an id given needs no vendor's letters");
}

/// Writes a crate from `files` with this checkout's crates, then builds and
/// tests it: the template compiles, its generated files match its
/// declarations, and its tests pass.
fn builds_and_passes_its_tests(name: &str, files: &[(&str, &str)]) {
    let dir = temp("e2e").join(name);
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let checkout = std::path::absolute(checkout).expect("a path");
    let hapto = checkout.to_string_lossy().replace('\\', "/");
    write(&dir, &identity(name), &hapto, files).expect("written");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = std::process::Command::new(cargo)
        .args(["test", "--quiet"])
        .env("CARGO_TARGET_DIR", checkout.join("target/hapto/init-e2e"))
        .current_dir(&dir)
        .status()
        .expect("cargo ran");
    assert!(status.success(), "the new crate's tests: {status}");
}

/// The gain the template writes builds and passes its tests. The full CI run
/// takes it on Windows, and by hand: `cargo test -p cargo-hapto --
/// --ignored`, minutes the first time, since the crate builds in a target
/// folder of its own.
#[test]
#[ignore = "builds a whole plugin crate"]
fn a_new_crate_builds_and_passes_its_tests() {
    builds_and_passes_its_tests("e2e-plugin", FILES);
}

/// The instrument `--instrument` writes builds and passes its tests: a note
/// sounds through the VST3 exporter until it is released.
#[test]
#[ignore = "builds a whole plugin crate"]
fn a_new_instrument_builds_and_passes_its_tests() {
    builds_and_passes_its_tests("e2e-synth", INSTRUMENT_FILES);
}

#[test]
fn the_next_step_quotes_a_folder_with_a_space_and_skips_the_one_it_ran_in() {
    use std::path::Path;
    assert_eq!(cd_line(Path::new("Tone Shaper")), "  cd \"Tone Shaper\"\n");
    assert_eq!(cd_line(Path::new("tone-shaper")), "  cd tone-shaper\n");
    assert_eq!(cd_line(Path::new(".")), "");
}
