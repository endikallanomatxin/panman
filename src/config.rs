use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const MANIFEST_DIR: &str = ".panman";
pub const MANIFEST_FILE: &str = "panman.toml";
pub const LOCK_FILE: &str = "panman.lock";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    #[serde(default)]
    pub environment: EnvironmentSection,

    #[serde(default)]
    pub packages: BTreeMap<String, PackageSpec>,

    #[serde(default)]
    pub shell: ShellSection,

    #[serde(default)]
    pub env: BTreeMap<String, String>,

    #[serde(default)]
    pub scripts: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentSection {
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShellSection {
    pub program: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageSource {
    Path,
    Archive,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PackageSpec {
    pub source: PackageSource,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,

    #[serde(default, skip_serializing_if = "is_zero")]
    pub strip_components: usize,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bins: Vec<PathBuf>,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

#[derive(Clone, Debug)]
pub struct ManifestLayer {
    pub file: PathBuf,
    pub project_root: PathBuf,
    pub manifest: Manifest,
}

#[derive(Clone, Debug)]
pub struct EffectivePackage {
    pub name: String,
    pub spec: PackageSpec,
    pub project_root: PathBuf,
    pub declared_in: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct EffectiveConfig {
    pub name: Option<String>,
    pub packages: BTreeMap<String, EffectivePackage>,
    pub shell_program: Option<String>,
    pub env: BTreeMap<String, String>,
    pub scripts: BTreeMap<String, String>,
    pub layers: Vec<PathBuf>,
}

impl EffectiveConfig {
    pub fn lock_path(&self) -> Option<PathBuf> {
        self.layers
            .last()
            .and_then(|file| file.parent())
            .map(|directory| directory.join(LOCK_FILE))
    }
}

pub fn discover(start: &Path) -> Result<Vec<ManifestLayer>> {
    let start = if start.is_file() {
        start
            .parent()
            .context("the supplied working path has no parent")?
            .to_path_buf()
    } else {
        start.to_path_buf()
    };

    let mut files = Vec::new();
    let mut current = Some(start.as_path());

    while let Some(directory) = current {
        let file = directory.join(MANIFEST_DIR).join(MANIFEST_FILE);
        if file.is_file() {
            files.push(file);
        }
        current = directory.parent();
    }

    files.reverse();
    files.into_iter().map(load_layer).collect()
}

fn load_layer(file: PathBuf) -> Result<ManifestLayer> {
    let contents = fs::read_to_string(&file)
        .with_context(|| format!("failed to read {}", file.display()))?;
    let manifest: Manifest = toml::from_str(&contents)
        .with_context(|| format!("failed to parse {}", file.display()))?;

    validate_manifest(&manifest)
        .with_context(|| format!("invalid manifest {}", file.display()))?;

    let panman_dir = file
        .parent()
        .context("manifest path has no parent directory")?;
    let project_root = panman_dir
        .parent()
        .unwrap_or(panman_dir)
        .to_path_buf();

    Ok(ManifestLayer {
        file,
        project_root,
        manifest,
    })
}

pub fn compose(layers: &[ManifestLayer]) -> EffectiveConfig {
    let mut effective = EffectiveConfig::default();

    for layer in layers {
        effective.layers.push(layer.file.clone());

        if let Some(name) = &layer.manifest.environment.name {
            effective.name = Some(name.clone());
        }
        if let Some(program) = &layer.manifest.shell.program {
            effective.shell_program = Some(program.clone());
        }

        effective.env.extend(layer.manifest.env.clone());
        effective.scripts.extend(layer.manifest.scripts.clone());

        for (name, spec) in &layer.manifest.packages {
            effective.packages.insert(
                name.clone(),
                EffectivePackage {
                    name: name.clone(),
                    spec: spec.clone(),
                    project_root: layer.project_root.clone(),
                    declared_in: layer.file.clone(),
                },
            );
        }
    }

    effective
}

pub fn load_effective(start: &Path) -> Result<EffectiveConfig> {
    let layers = discover(start)?;
    if layers.is_empty() {
        bail!(
            "no {} found from {} or any parent directory",
            MANIFEST_FILE,
            start.display()
        );
    }
    Ok(compose(&layers))
}

pub fn load_effective_or_empty(start: &Path) -> Result<EffectiveConfig> {
    Ok(compose(&discover(start)?))
}

pub fn validate_manifest(manifest: &Manifest) -> Result<()> {
    for (name, package) in &manifest.packages {
        if name.trim().is_empty() {
            bail!("package names cannot be empty");
        }

        match package.source {
            PackageSource::Path => {
                if package.path.is_none() {
                    bail!("package {name:?} uses source=path but has no path");
                }
                if package.url.is_some() || package.sha256.is_some() {
                    bail!("package {name:?} cannot combine path with url or sha256");
                }
            }
            PackageSource::Archive => {
                if package.url.is_none() || package.sha256.is_none() {
                    bail!("package {name:?} uses source=archive and requires url and sha256");
                }
                if package.path.is_some() {
                    bail!("package {name:?} cannot combine archive with path");
                }
            }
        }
    }

    Ok(())
}

pub fn nearest_manifest(start: &Path) -> Result<PathBuf> {
    discover(start)?
        .last()
        .map(|layer| layer.file.clone())
        .context("no panman.toml found; run `panman init` first")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn child_layer_overrides_parent() {
        let temp = tempdir().unwrap();
        let parent = temp.path();
        let child = parent.join("project");
        fs::create_dir_all(parent.join(".panman")).unwrap();
        fs::create_dir_all(child.join(".panman")).unwrap();

        fs::write(
            parent.join(".panman/panman.toml"),
            "[env]\nEDITOR = 'vim'\nSHARED = 'parent'\n",
        )
        .unwrap();
        fs::write(
            child.join(".panman/panman.toml"),
            "[env]\nEDITOR = 'nvim'\nPROJECT = 'yes'\n",
        )
        .unwrap();

        let layers = discover(&child).unwrap();
        let effective = compose(&layers);

        assert_eq!(layers.len(), 2);
        assert_eq!(effective.env["EDITOR"], "nvim");
        assert_eq!(effective.env["SHARED"], "parent");
        assert_eq!(effective.env["PROJECT"], "yes");
    }
}
