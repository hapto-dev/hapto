//! The workspace a command runs in, as `cargo metadata` describes it, and the
//! plugin crates in it.

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use crate::tool::{cargo, output};

/// The workspace around the folder a command started in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Workspace {
    /// The folder of the workspace's root manifest.
    pub(crate) root: PathBuf,
    /// The folder Cargo builds into.
    pub(crate) target: PathBuf,
    /// Its members, in the order Cargo lists them.
    pub(crate) packages: Vec<Package>,
}

/// One member of the workspace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Package {
    /// The package's name, as `-p` takes it.
    pub(crate) name: String,
    /// The folder of its manifest.
    pub(crate) dir: PathBuf,
    /// Its build targets.
    pub(crate) targets: Vec<Target>,
    /// The name every host shows, from `[package.metadata.hapto]`.
    pub(crate) hapto_name: Option<String>,
}

/// A build target of a package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    /// Its name, which names the file the build writes.
    pub(crate) name: String,
    /// Its kinds: `lib`, `cdylib`, `bin`, `test` and so on.
    pub(crate) kinds: Vec<String>,
    /// The features a build of it needs.
    pub(crate) required_features: Vec<String>,
}

impl Target {
    fn is(&self, kind: &str) -> bool {
        self.kinds.iter().any(|k| k == kind)
    }
}

/// A plugin crate: a member that builds a `cdylib` and names the plugin in
/// `[package.metadata.hapto]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Plugin {
    /// The package's name.
    pub(crate) package: String,
    /// The name every host shows; the bundle's name too.
    pub(crate) name: String,
    /// The `cdylib`'s name, with Cargo's `_` for `-`, as the file it writes.
    pub(crate) lib: String,
    /// The crate's folder.
    pub(crate) dir: PathBuf,
    /// The binary that runs the plugin standalone, when the crate has one:
    /// a `bin` target that needs the crate's `standalone` feature.
    pub(crate) standalone: Option<String>,
    /// Whether the crate has the `page_params` test, which rewrites the
    /// page's parameter map when run with `HAPTO_BLESS=1`.
    pub(crate) has_param_map_test: bool,
    /// Whether the crate has the `juce_settings` test, which rewrites its
    /// JUCE adapter settings when run with `HAPTO_BLESS=1`.
    pub(crate) has_juce_settings_test: bool,
}

impl Plugin {
    /// The npm package of the plugin's page, when it has one.
    pub(crate) fn page(&self) -> Option<PathBuf> {
        let page = self.dir.join("page");
        page.join("package.json").is_file().then_some(page)
    }
}

impl Workspace {
    /// Reads the workspace around `dir` through `cargo metadata`.
    ///
    /// # Errors
    ///
    /// Returns what Cargo said when it read none, such as no manifest above
    /// `dir`.
    pub(crate) fn read(dir: &Path) -> Result<Workspace> {
        let stdout = output(
            cargo()
                .args(["metadata", "--format-version", "1", "--no-deps"])
                .current_dir(dir),
            "cargo metadata",
        )?;
        let metadata = serde_json::from_str(&stdout).context("cargo metadata")?;
        let workspace = Workspace::from_metadata(&metadata)?;
        workspace.warn_of_another_version();
        Ok(workspace)
    }

    /// Warns when the workspace's lockfile pins a `hapto-host` of another
    /// version than this command's: a bundle's layout, the page's dev server
    /// and the standalone app are each this command's idea of them, and
    /// another version's may differ.
    fn warn_of_another_version(&self) {
        let Ok(lockfile) = std::fs::read_to_string(self.root.join("Cargo.lock")) else {
            return;
        };
        let ours = env!("CARGO_PKG_VERSION");
        if let Some(locked) = locked_version(&lockfile, "hapto-host")
            && locked != ours
        {
            eprintln!(
                "warning: this workspace builds hapto-host {locked}, and this cargo-hapto is \
                 {ours}; install the cargo-hapto of the same version"
            );
        }
    }

    /// Reads the output of `cargo metadata --format-version 1 --no-deps`.
    ///
    /// # Errors
    ///
    /// Returns which field the output lacks.
    pub(crate) fn from_metadata(metadata: &serde_json::Value) -> Result<Workspace> {
        let text = |key: &str| {
            metadata[key]
                .as_str()
                .map(PathBuf::from)
                .with_context(|| format!("cargo metadata gave no {key}"))
        };
        let packages = metadata["packages"]
            .as_array()
            .context("cargo metadata gave no packages")?
            .iter()
            .map(package)
            .collect::<Result<_, _>>()?;
        Ok(Workspace {
            root: text("workspace_root")?,
            target: text("target_directory")?,
            packages,
        })
    }

    /// Returns the folder of every member, which `dev` watches.
    pub(crate) fn member_dirs(&self) -> impl Iterator<Item = &Path> {
        self.packages.iter().map(|p| p.dir.as_path())
    }

    /// Reads the workspace around the current folder, and the plugin a
    /// command works on there, as [`plugin`](Self::plugin) finds it.
    ///
    /// # Errors
    ///
    /// Returns why there is no workspace, or no plugin to work on.
    pub(crate) fn find(package: Option<&str>) -> Result<(Workspace, Plugin)> {
        let dir = std::env::current_dir().context("the current folder")?;
        let workspace = Workspace::read(&dir)?;
        let plugin = workspace.plugin(package, &dir)?;
        Ok((workspace, plugin))
    }

    /// Returns the plugin a command works on: the one `package` names, else
    /// the plugin crate `dir` is inside, else the workspace's only one.
    ///
    /// # Errors
    ///
    /// Returns why there is no plugin to work on, or which ones there are to
    /// choose from with `-p`.
    pub(crate) fn plugin(&self, package: Option<&str>, dir: &Path) -> Result<Plugin> {
        if let Some(name) = package {
            let found = self
                .packages
                .iter()
                .find(|p| p.name == name)
                .with_context(|| format!("{name} is not a crate of this workspace"))?;
            return found.plugin();
        }
        let plugins: Vec<Plugin> = self
            .packages
            .iter()
            .filter_map(|p| p.plugin().ok())
            .collect();
        // The deepest plugin folder `dir` is in, so a plugin nested in
        // another crate's folder is found from its own.
        let around = plugins
            .iter()
            .filter(|p| dir.starts_with(&p.dir))
            .max_by_key(|p| p.dir.components().count());
        if let Some(plugin) = around {
            return Ok(plugin.clone());
        }
        match plugins.as_slice() {
            [only] => Ok(only.clone()),
            [] => bail!(
                "this workspace has no plugin crate: a crate that builds a cdylib \
                 and names the plugin in [package.metadata.hapto]"
            ),
            many => {
                let names: Vec<&str> = many.iter().map(|p| p.package.as_str()).collect();
                bail!(
                    "this workspace has {} plugin crates; pick one with -p: {}",
                    many.len(),
                    names.join(", ")
                )
            }
        }
    }
}

impl Package {
    /// Returns the package as a plugin.
    ///
    /// # Errors
    ///
    /// Returns why it is none: no `cdylib`, or no name in
    /// `[package.metadata.hapto]`.
    pub(crate) fn plugin(&self) -> Result<Plugin> {
        let lib = self
            .targets
            .iter()
            .find(|t| t.is("cdylib"))
            .with_context(|| format!("{} builds no cdylib, so it has no module", self.name))?;
        let name = self
            .hapto_name
            .clone()
            .with_context(|| format!("{} has no name in [package.metadata.hapto]", self.name))?;
        let standalone = self
            .targets
            .iter()
            .find(|t| t.is("bin") && t.required_features.iter().any(|f| f == "standalone"))
            .map(|t| t.name.clone());
        let has_test = |name: &str| self.targets.iter().any(|t| t.is("test") && t.name == name);
        let has_param_map_test = has_test("page_params");
        let has_juce_settings_test = has_test("juce_settings");
        Ok(Plugin {
            package: self.name.clone(),
            name,
            lib: lib.name.replace('-', "_"),
            dir: self.dir.clone(),
            standalone,
            has_param_map_test,
            has_juce_settings_test,
        })
    }
}

/// Returns the version `lockfile`, a `Cargo.lock`, pins `package` at: the
/// `version` line after its `name` line, which Cargo writes next to each
/// other. `None` when it pins none, or more than one.
pub(crate) fn locked_version<'a>(lockfile: &'a str, package: &str) -> Option<&'a str> {
    let name = format!("name = \"{package}\"");
    let mut lines = lockfile.lines().map(str::trim);
    let mut found = None;
    while lines.any(|line| line == name) {
        let version = lines
            .next()?
            .strip_prefix("version = \"")?
            .strip_suffix('"')?;
        if found.replace(version).is_some() {
            return None;
        }
    }
    found
}

fn package(value: &serde_json::Value) -> Result<Package> {
    let name = value["name"]
        .as_str()
        .context("cargo metadata gave a package no name")?;
    let manifest = value["manifest_path"]
        .as_str()
        .with_context(|| format!("cargo metadata gave {name} no manifest_path"))?;
    let dir = Path::new(manifest)
        .parent()
        .context("a manifest path has no folder")?;
    let strings = |value: &serde_json::Value| -> Vec<String> {
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s.as_str().map(str::to_owned))
            .collect()
    };
    let targets = value["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            Some(Target {
                name: t["name"].as_str()?.to_owned(),
                kinds: strings(&t["kind"]),
                required_features: strings(&t["required-features"]),
            })
        })
        .collect();
    Ok(Package {
        name: name.to_owned(),
        dir: dir.to_path_buf(),
        targets,
        hapto_name: value["metadata"]["hapto"]["name"]
            .as_str()
            .map(str::to_owned),
    })
}
