//! Read-only Cargo patch overlays for explicit local workspaces, including glob members.
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// Enumerate published libraries by reading manifests only; no Cargo resolution mutates a lockfile.
fn packages(root: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let root = root.canonicalize()?;
    let workspace: toml::Value = toml::from_str(&fs::read_to_string(root.join("Cargo.toml"))?)?;
    let patterns = match workspace.get("workspace").and_then(|workspace| workspace.get("members")) {
        Some(members) => members
            .as_array()
            .context("workspace members must be an array")?
            .iter()
            .map(|value| value.as_str().context("workspace member must be a string"))
            .collect::<Result<Vec<_>>>()?,
        None => vec!["."],
    };
    let mut packages = BTreeMap::new();
    for pattern in patterns {
        let pattern = root.join(pattern);
        for directory in glob::glob(pattern.to_str().context("workspace member path is not UTF-8")?)? {
            let directory = directory?.canonicalize()?;
            let manifest: toml::Value = toml::from_str(&fs::read_to_string(directory.join("Cargo.toml"))?)?;
            let Some(package) = manifest.get("package") else {
                continue;
            };
            let private = package
                .get("publish")
                .is_some_and(|publish| publish.as_bool() == Some(false) || publish.as_array().is_some_and(Vec::is_empty));
            if private || (manifest.get("lib").is_none() && !directory.join("src/lib.rs").is_file()) {
                continue;
            }
            let name = package.get("name").and_then(toml::Value::as_str).context("library needs a package name")?;
            if let Some(previous) = packages.insert(name.to_owned(), directory.clone()) {
                ensure!(previous == directory, "conflicting source for {name}");
            }
        }
    }
    Ok(packages)
}

/// Serialize paths through TOML rather than interpolating quoting or modifying source manifests.
pub(crate) fn overlay(root: &Path) -> Result<String> {
    let mut patches = toml::map::Map::new();
    for (name, path) in packages(root)? {
        let mut dependency = toml::map::Map::new();
        dependency.insert(
            "path".into(),
            toml::Value::String(path.to_str().context("package path is not UTF-8")?.to_owned()),
        );
        patches.insert(name, toml::Value::Table(dependency));
    }
    let mut registry = toml::map::Map::new();
    registry.insert("crates-io".into(), toml::Value::Table(patches));
    let mut document = toml::map::Map::new();
    document.insert("patch".into(), toml::Value::Table(registry));
    Ok(toml::to_string(&document)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Private tools, binary applications and unpublished fixtures never enter downstream patches.
    #[test]
    fn selects_libraries_and_preserves_manifest_and_lock() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("author's checkout");
        fs::create_dir(&root).unwrap();
        let manifest = "[workspace]\nmembers=['crates/*']\n";
        fs::write(root.join("Cargo.toml"), manifest).unwrap();
        fs::write(root.join("Cargo.lock"), "untouched").unwrap();
        for (name, publication, library) in [
            ("sdk", "", true),
            ("private", "publish=false", true),
            ("fixture", "publish=[]", true),
            ("application", "", false),
        ] {
            let path = root.join("crates").join(name);
            fs::create_dir_all(path.join("src")).unwrap();
            fs::write(path.join("Cargo.toml"), format!("[package]\nname='{name}'\nversion='0.0.0'\n{publication}\n")).unwrap();
            fs::write(path.join(if library { "src/lib.rs" } else { "src/main.rs" }), "").unwrap();
        }
        let overlay: toml::Value = toml::from_str(&overlay(&root).unwrap()).unwrap();
        let patches = overlay["patch"]["crates-io"].as_table().unwrap();
        assert_eq!(patches.len(), 1);
        assert_eq!(
            patches["sdk"]["path"].as_str().unwrap(),
            root.join("crates/sdk").canonicalize().unwrap().to_str().unwrap()
        );
        assert_eq!(fs::read_to_string(root.join("Cargo.toml")).unwrap(), manifest);
        assert_eq!(fs::read_to_string(root.join("Cargo.lock")).unwrap(), "untouched");
    }

    /// An ordinary non-workspace library remains usable, and duplicate names cannot hide a source.
    #[test]
    fn standalone_library_and_conflicting_names() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("Cargo.toml"),
            "[package]\nname='sdk'\nversion='0.0.0'\n[lib]\npath='lib.rs'\n",
        )
        .unwrap();
        assert_eq!(packages(directory.path()).unwrap().len(), 1);
        fs::write(directory.path().join("Cargo.toml"), "[workspace]\nmembers=['a','b']\n").unwrap();
        for name in ["a", "b"] {
            let path = directory.path().join(name);
            fs::create_dir(&path).unwrap();
            fs::write(path.join("Cargo.toml"), "[package]\nname='same'\nversion='0.0.0'\n[lib]\n").unwrap();
        }
        assert!(packages(directory.path()).unwrap_err().to_string().contains("conflicting source"));
    }
}
