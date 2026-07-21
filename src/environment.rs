use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tempfile::Builder;

use crate::config::EffectiveConfig;
use crate::store::{LockFile, LockedPackage, StoreLocations};

#[derive(Serialize)]
struct EnvironmentIdentity<'a> {
    packages: &'a BTreeMap<String, LockedPackage>,
    env: &'a BTreeMap<String, String>,
    shell: &'a Option<String>,
}

#[derive(Serialize)]
struct EnvironmentMetadata<'a> {
    hash: &'a str,
    target: &'a str,
    manifests: &'a [PathBuf],
    packages: &'a BTreeMap<String, LockedPackage>,
    env: &'a BTreeMap<String, String>,
    shell: &'a Option<String>,
}

pub fn environment_hash(
    packages: &BTreeMap<String, LockedPackage>,
    config: &EffectiveConfig,
) -> Result<String> {
    let identity = EnvironmentIdentity {
        packages,
        env: &config.env,
        shell: &config.shell_program,
    };
    let encoded = serde_json::to_vec(&identity)?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

pub fn build(
    hash: &str,
    lock: &LockFile,
    config: &EffectiveConfig,
    locations: &StoreLocations,
) -> Result<PathBuf> {
    let final_path = locations.environments.join(&hash[..24]);
    if final_path.join("environment.json").is_file() {
        return Ok(final_path);
    }

    fs::create_dir_all(&locations.environments)?;
    let temp = Builder::new()
        .prefix(".panman-environment-")
        .tempdir_in(&locations.environments)?;
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir)?;

    let mut exposed = BTreeMap::<String, PathBuf>::new();
    for (package_name, package) in &lock.packages {
        let root = PathBuf::from(&package.store_path);
        for relative in &package.bins {
            let executable = root.join(relative);
            let name = executable
                .file_name()
                .context("declared executable has no file name")?
                .to_string_lossy()
                .into_owned();

            if let Some(previous) = exposed.get(&name) {
                bail!(
                    "executable collision for {name:?}: {} and package {package_name:?}",
                    previous.display()
                );
            }
            expose_executable(&executable, &bin_dir, &name)?;
            exposed.insert(name, executable);
        }
    }

    let metadata = EnvironmentMetadata {
        hash,
        target: &lock.target,
        manifests: &config.layers,
        packages: &lock.packages,
        env: &config.env,
        shell: &config.shell_program,
    };
    fs::write(
        temp.path().join("environment.json"),
        serde_json::to_vec_pretty(&metadata)?,
    )?;

    let temp_path = temp.keep();
    match fs::rename(&temp_path, &final_path) {
        Ok(()) => Ok(final_path),
        Err(_error) if final_path.exists() => {
            fs::remove_dir_all(&temp_path).ok();
            Ok(final_path)
        }
        Err(error) => Err(error)
            .with_context(|| format!("failed to create environment {}", final_path.display())),
    }
}

#[cfg(unix)]
fn expose_executable(source: &Path, bin_dir: &Path, name: &str) -> Result<()> {
    use std::os::unix::fs::symlink;

    let destination = bin_dir.join(name);
    symlink(source, &destination).with_context(|| {
        format!(
            "failed to link {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

#[cfg(windows)]
fn expose_executable(source: &Path, bin_dir: &Path, name: &str) -> Result<()> {
    let source = source.canonicalize()?;
    if source
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        let destination = bin_dir.join(name);
        if fs::hard_link(&source, &destination).is_err() {
            fs::copy(&source, &destination)?;
        }
        return Ok(());
    }

    let stem = name
        .strip_suffix(".cmd")
        .or_else(|| name.strip_suffix(".bat"))
        .unwrap_or(name);
    let destination = bin_dir.join(format!("{stem}.cmd"));
    let escaped = source.to_string_lossy().replace('"', "\"\"");
    fs::write(
        &destination,
        format!("@echo off\r\ncall \"{escaped}\" %*\r\n"),
    )?;
    Ok(())
}
