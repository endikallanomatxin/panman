use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::Builder;
use walkdir::WalkDir;
use zip::ZipArchive;

use crate::config::{EffectiveConfig, EffectivePackage, PackageSource};
use crate::environment;
use crate::platform;

#[derive(Clone, Debug)]
pub struct StoreLocations {
    pub user_root: PathBuf,
    pub user_store: PathBuf,
    pub system_store: PathBuf,
    pub environments: PathBuf,
    pub cache: PathBuf,
}

impl StoreLocations {
    pub fn discover() -> Result<Self> {
        let user_root = platform::panman_home()?;
        let system_root = platform::system_home()?;
        Ok(Self {
            user_store: user_root.join("store"),
            system_store: system_root.join("store"),
            environments: user_root.join("environments"),
            cache: user_root.join("cache"),
            user_root,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LockFile {
    pub version: u32,
    pub target: String,
    pub environment_hash: String,
    pub packages: BTreeMap<String, LockedPackage>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LockedPackage {
    pub digest: String,
    pub source: String,
    pub store_path: String,
    pub bins: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct PackageMetadata {
    name: String,
    digest: String,
    source: String,
    declared_in: String,
    bins: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SyncResult {
    pub environment_dir: PathBuf,
    pub bin_dir: PathBuf,
    pub lock_path: PathBuf,
    pub config: EffectiveConfig,
    pub lock: LockFile,
}

pub fn sync(config: EffectiveConfig, locations: &StoreLocations, system: bool) -> Result<SyncResult> {
    fs::create_dir_all(&locations.user_root)
        .with_context(|| format!("failed to create {}", locations.user_root.display()))?;
    fs::create_dir_all(&locations.cache)
        .with_context(|| format!("failed to create {}", locations.cache.display()))?;
    fs::create_dir_all(&locations.environments)
        .with_context(|| format!("failed to create {}", locations.environments.display()))?;

    let write_store = if system {
        &locations.system_store
    } else {
        &locations.user_store
    };
    fs::create_dir_all(write_store)
        .with_context(|| format!("failed to create store {}", write_store.display()))?;

    let lock_handle = open_store_lock(write_store)?;
    lock_handle
        .lock_exclusive()
        .with_context(|| format!("failed to lock store {}", write_store.display()))?;

    let mut packages = BTreeMap::new();
    for package in config.packages.values() {
        let resolved = resolve_package(package, locations, write_store)?;
        packages.insert(package.name.clone(), resolved);
    }

    FileExt::unlock(&lock_handle).ok();

    let environment_hash = environment::environment_hash(&packages, &config)?;
    let lock = LockFile {
        version: 1,
        target: platform::target_name(),
        environment_hash: environment_hash.clone(),
        packages,
    };

    let environment_dir = environment::build(&environment_hash, &lock, &config, locations)?;
    let lock_path = config
        .lock_path()
        .context("cannot write a lockfile without at least one manifest")?;
    write_toml_atomic(&lock_path, &lock)?;

    Ok(SyncResult {
        bin_dir: environment_dir.join("bin"),
        environment_dir,
        lock_path,
        config,
        lock,
    })
}

fn open_store_lock(store: &Path) -> Result<File> {
    let path = store.join(".panman-store.lock");
    File::options()
        .create(true)
        .read(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("failed to open {}", path.display()))
}

fn resolve_package(
    package: &EffectivePackage,
    locations: &StoreLocations,
    write_store: &Path,
) -> Result<LockedPackage> {
    let (digest, source) = match package.spec.source {
        PackageSource::Path => {
            let configured = package
                .spec
                .path
                .as_ref()
                .context("path package has no path")?;
            let source_path = absolutize(&package.project_root, configured);
            if !source_path.is_dir() {
                bail!(
                    "path package {:?} does not point to a directory: {}",
                    package.name,
                    source_path.display()
                );
            }
            (hash_tree(&source_path)?, format!("path:{}", source_path.display()))
        }
        PackageSource::Archive => {
            let digest = normalize_sha256(
                package
                    .spec
                    .sha256
                    .as_deref()
                    .context("archive package has no sha256")?,
            )?;
            let url = package
                .spec
                .url
                .as_ref()
                .context("archive package has no url")?;
            (digest, format!("archive:{url}"))
        }
    };

    let directory_name = format!("{}-{}", &digest[..16], sanitize_name(&package.name));
    let system_path = locations.system_store.join(&directory_name);
    let user_path = locations.user_store.join(&directory_name);

    let store_path = if system_path.is_dir() {
        system_path
    } else if user_path.is_dir() {
        user_path
    } else {
        let final_path = write_store.join(&directory_name);
        install_package(package, &digest, &source, &final_path, locations)?;
        final_path
    };

    for bin in &package.spec.bins {
        let path = store_path.join(bin);
        if !path.is_file() {
            bail!(
                "package {:?} declares missing executable {}",
                package.name,
                path.display()
            );
        }
    }

    Ok(LockedPackage {
        digest,
        source,
        store_path: store_path.to_string_lossy().into_owned(),
        bins: package
            .spec
            .bins
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
    })
}

fn install_package(
    package: &EffectivePackage,
    digest: &str,
    source: &str,
    final_path: &Path,
    locations: &StoreLocations,
) -> Result<()> {
    if final_path.exists() {
        return Ok(());
    }

    let parent = final_path
        .parent()
        .context("store package path has no parent")?;
    let temp = Builder::new()
        .prefix(".panman-import-")
        .tempdir_in(parent)
        .with_context(|| format!("failed to create a transaction in {}", parent.display()))?;

    match package.spec.source {
        PackageSource::Path => {
            let configured = package
                .spec
                .path
                .as_ref()
                .context("path package has no path")?;
            let source_path = absolutize(&package.project_root, configured);
            copy_tree(&source_path, temp.path())?;
            let copied_digest = hash_tree(temp.path())?;
            if copied_digest != digest {
                bail!(
                    "path package {:?} changed while it was being imported",
                    package.name
                );
            }
        }
        PackageSource::Archive => {
            let url = package
                .spec
                .url
                .as_ref()
                .context("archive package has no url")?;
            let archive = fetch_archive(url, digest, &locations.cache)?;
            extract_archive(
                &archive,
                url,
                temp.path(),
                package.spec.strip_components,
            )?;
        }
    }

    let metadata = PackageMetadata {
        name: package.name.clone(),
        digest: digest.to_owned(),
        source: source.to_owned(),
        declared_in: package.declared_in.to_string_lossy().into_owned(),
        bins: package
            .spec
            .bins
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
    };
    let metadata_path = temp.path().join(".panman-package.json");
    fs::write(&metadata_path, serde_json::to_vec_pretty(&metadata)?)
        .with_context(|| format!("failed to write {}", metadata_path.display()))?;

    let temp_path = temp.keep();
    match fs::rename(&temp_path, final_path) {
        Ok(()) => Ok(()),
        Err(_error) if final_path.exists() => {
            fs::remove_dir_all(&temp_path).ok();
            Ok(())
        }
        Err(error) => Err(error)
            .with_context(|| format!("failed to commit package into {}", final_path.display())),
    }
}

fn fetch_archive(url: &str, digest: &str, cache: &Path) -> Result<PathBuf> {
    let archives = cache.join("archives");
    fs::create_dir_all(&archives)
        .with_context(|| format!("failed to create {}", archives.display()))?;
    let cached = archives.join(digest);

    if cached.is_file() {
        let actual = hash_file(&cached)?;
        if actual == digest {
            return Ok(cached);
        }
        fs::remove_file(&cached).ok();
    }

    let temporary = archives.join(format!(".{digest}.download"));
    let mut response = reqwest::blocking::Client::builder()
        .user_agent(concat!("panman/", env!("CARGO_PKG_VERSION")))
        .build()?
        .get(url)
        .send()
        .with_context(|| format!("failed to download {url}"))?
        .error_for_status()
        .with_context(|| format!("download failed for {url}"))?;

    let mut file = File::create(&temporary)
        .with_context(|| format!("failed to create {}", temporary.display()))?;
    io::copy(&mut response, &mut file)
        .with_context(|| format!("failed to save download from {url}"))?;
    file.sync_all().ok();

    let actual = hash_file(&temporary)?;
    if actual != digest {
        fs::remove_file(&temporary).ok();
        bail!("sha256 mismatch for {url}: expected {digest}, got {actual}");
    }

    fs::rename(&temporary, &cached)
        .with_context(|| format!("failed to cache archive at {}", cached.display()))?;
    Ok(cached)
}

fn extract_archive(
    archive_path: &Path,
    url: &str,
    destination: &Path,
    strip_components: usize,
) -> Result<()> {
    let raw = tempfile::tempdir().context("failed to create extraction directory")?;
    let lower = url.split('?').next().unwrap_or(url).to_ascii_lowercase();

    if lower.ends_with(".zip") {
        extract_zip(archive_path, raw.path())?;
    } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        let file = File::open(archive_path)?;
        extract_tar(GzDecoder::new(BufReader::new(file)), raw.path())?;
    } else if lower.ends_with(".tar") {
        let file = File::open(archive_path)?;
        extract_tar(BufReader::new(file), raw.path())?;
    } else {
        bail!("unsupported archive format for {url}; expected .zip, .tar, .tar.gz or .tgz");
    }

    copy_tree_stripped(raw.path(), destination, strip_components)
}

fn extract_tar<R: Read>(reader: R, destination: &Path) -> Result<()> {
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries().context("failed to read tar archive")? {
        let mut entry = entry.context("failed to read tar entry")?;
        let unpacked = entry
            .unpack_in(destination)
            .context("failed to unpack tar entry")?;
        if !unpacked {
            bail!("tar archive contains an unsafe path");
        }
    }
    Ok(())
}

fn extract_zip(archive_path: &Path, destination: &Path) -> Result<()> {
    let file = File::open(archive_path)?;
    let mut archive = ZipArchive::new(file).context("failed to read zip archive")?;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let relative = entry
            .enclosed_name()
            .context("zip archive contains an unsafe path")?;
        let output = destination.join(relative);

        if entry.is_dir() {
            fs::create_dir_all(&output)?;
            continue;
        }

        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = File::create(&output)?;
        io::copy(&mut entry, &mut file)?;

        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&output, fs::Permissions::from_mode(mode))?;
        }
    }

    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    copy_tree_stripped(source, destination, 0)
}

fn copy_tree_stripped(source: &Path, destination: &Path, strip_components: usize) -> Result<()> {
    let mut entries = WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path().to_path_buf());

    for entry in entries {
        if entry.path() == source {
            continue;
        }
        let relative = entry.path().strip_prefix(source)?;
        let stripped = strip_path(relative, strip_components);
        let Some(stripped) = stripped else {
            continue;
        };
        let output = destination.join(stripped);

        if entry.file_type().is_dir() {
            fs::create_dir_all(&output)?;
        } else if entry.file_type().is_symlink() {
            copy_symlink(entry.path(), &output)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &output).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    entry.path().display(),
                    output.display()
                )
            })?;
            fs::set_permissions(&output, fs::metadata(entry.path())?.permissions())?;
        }
    }

    Ok(())
}

fn strip_path(path: &Path, count: usize) -> Option<PathBuf> {
    let components: Vec<_> = path.components().collect();
    if components.len() <= count {
        return None;
    }

    let mut output = PathBuf::new();
    for component in components.into_iter().skip(count) {
        match component {
            Component::Normal(value) => output.push(value),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!output.as_os_str().is_empty()).then_some(output)
}

#[cfg(unix)]
fn copy_symlink(source: &Path, destination: &Path) -> Result<()> {
    use std::os::unix::fs::symlink;

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    symlink(fs::read_link(source)?, destination)?;
    Ok(())
}

#[cfg(windows)]
fn copy_symlink(source: &Path, destination: &Path) -> Result<()> {
    let target = fs::canonicalize(source)
        .with_context(|| format!("failed to resolve symlink {}", source.display()))?;
    if target.is_dir() {
        copy_tree(&target, destination)
    } else {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(target, destination)?;
        Ok(())
    }
}

fn hash_tree(root: &Path) -> Result<String> {
    let mut entries = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path().to_path_buf());

    let mut hasher = Sha256::new();
    for entry in entries {
        if entry.path() == root {
            continue;
        }
        let relative = entry.path().strip_prefix(root)?;
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update([0]);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            hasher.update(fs::symlink_metadata(entry.path())?.permissions().mode().to_le_bytes());
        }

        if entry.file_type().is_dir() {
            hasher.update(b"directory\0");
        } else if entry.file_type().is_symlink() {
            hasher.update(b"symlink\0");
            hasher.update(fs::read_link(entry.path())?.to_string_lossy().as_bytes());
        } else {
            hasher.update(b"file\0");
            let mut file = File::open(entry.path())?;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
        }
        hasher.update([0xff]);
    }

    Ok(hex::encode(hasher.finalize()))
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn normalize_sha256(value: &str) -> Result<String> {
    let value = value
        .trim()
        .strip_prefix("sha256:")
        .unwrap_or(value.trim())
        .to_ascii_lowercase();
    if value.len() != 64 || !value.chars().all(|character| character.is_ascii_hexdigit()) {
        bail!("invalid sha256 digest {value:?}");
    }
    Ok(value)
}

fn sanitize_name(name: &str) -> String {
    let output: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect();
    if output.is_empty() {
        "package".to_owned()
    } else {
        output
    }
}

fn absolutize(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn write_toml_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().context("lockfile path has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .unwrap_or(OsStr::new("lock"))
            .to_string_lossy()
    ));
    let contents = toml::to_string_pretty(value)?;
    let mut file = File::create(&temporary)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all().ok();
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(&temporary, path)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}
