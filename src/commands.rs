use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitStatus};

use anyhow::{bail, Context, Result};

use crate::cli::{Cli, Command, ShellCommand};
use crate::config::{
    self, EffectiveConfig, Manifest, PackageSource, PackageSpec, MANIFEST_DIR, MANIFEST_FILE,
};
use crate::platform;
use crate::store::{self, StoreLocations, SyncResult};

const SHELL_BLOCK_START: &str = "# >>> PANMAN SHELL >>>";
const SHELL_BLOCK_END: &str = "# <<< PANMAN SHELL <<<";

pub fn execute(cli: Cli) -> Result<()> {
    let cwd = match cli.cwd {
        Some(path) => path,
        None => env::current_dir().context("failed to determine current directory")?,
    };

    match cli.command {
        Command::Init { force } => init(&cwd, force),
        Command::Add {
            name,
            path,
            url,
            sha256,
            strip_components,
            bins,
        } => add(
            &cwd,
            name,
            path,
            url,
            sha256,
            strip_components,
            bins,
        ),
        Command::Sync { system } => {
            let result = sync_current(&cwd, system, false)?;
            print_sync_result(&result);
            Ok(())
        }
        Command::Sh { auto, system } => shell(&cwd, auto, system),
        Command::Run { system, command } => run(&cwd, system, command),
        Command::Status => status(&cwd),
        Command::Shell { command } => match command {
            ShellCommand::Enable => enable_shell(),
            ShellCommand::Disable => disable_shell(),
        },
        Command::SelfInstall => self_install().map(|_| ()),
    }
}

fn init(cwd: &Path, force: bool) -> Result<()> {
    let directory = cwd.join(MANIFEST_DIR);
    let manifest_path = directory.join(MANIFEST_FILE);
    if manifest_path.exists() && !force {
        bail!(
            "{} already exists; pass --force to replace it",
            manifest_path.display()
        );
    }

    fs::create_dir_all(&directory)?;
    let name = cwd
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("development");
    let template = format!(
        r#"[environment]
name = {name:?}

# Panman starts this shell after composing all parent and project manifests.
# [shell]
# program = "zsh"

[env]
# EDITOR = "nvim"

[scripts]
# test = "cargo test"

# Packages can initially come from a local directory or a verified archive.
# [packages.example]
# source = "path"
# path = "./tools/example"
# bins = ["bin/example"]
"#
    );
    fs::write(&manifest_path, template)?;
    println!("created {}", manifest_path.display());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn add(
    cwd: &Path,
    name: String,
    path: Option<PathBuf>,
    url: Option<String>,
    sha256: Option<String>,
    strip_components: usize,
    bins: Vec<PathBuf>,
) -> Result<()> {
    if name.trim().is_empty() {
        bail!("package name cannot be empty");
    }

    let manifest_path = config::nearest_manifest(cwd)?;
    let contents = fs::read_to_string(&manifest_path)?;
    let mut manifest: Manifest = toml::from_str(&contents)?;

    let spec = match (path, url) {
        (Some(path), None) => PackageSpec {
            source: PackageSource::Path,
            path: Some(path),
            url: None,
            sha256: None,
            strip_components: 0,
            bins,
        },
        (None, Some(url)) => PackageSpec {
            source: PackageSource::Archive,
            path: None,
            url: Some(url),
            sha256: Some(sha256.context("--sha256 is required with --url")?),
            strip_components,
            bins,
        },
        (None, None) => bail!("provide exactly one of --path or --url"),
        (Some(_), Some(_)) => unreachable!("clap prevents path and url together"),
    };

    manifest.packages.insert(name.clone(), spec);
    config::validate_manifest(&manifest)?;
    write_atomic(&manifest_path, toml::to_string_pretty(&manifest)?.as_bytes())?;
    println!("updated {} with package {name:?}", manifest_path.display());
    Ok(())
}

fn status(cwd: &Path) -> Result<()> {
    let layers = config::discover(cwd)?;
    let effective = config::compose(&layers);
    let locations = StoreLocations::discover()?;

    println!("target: {}", platform::target_name());
    println!("user store: {}", locations.user_store.display());
    println!("system store: {}", locations.system_store.display());

    if layers.is_empty() {
        println!("manifests: none");
        return Ok(());
    }

    println!("manifests:");
    for layer in &layers {
        println!("  - {}", layer.file.display());
    }

    println!("packages: {}", effective.packages.len());
    for (name, package) in &effective.packages {
        println!("  - {name} ({})", package.declared_in.display());
    }

    println!(
        "shell: {}",
        effective
            .shell_program
            .as_deref()
            .unwrap_or("<system default>")
    );
    println!("environment variables: {}", effective.env.len());
    println!("scripts: {}", effective.scripts.len());
    Ok(())
}

fn sync_current(cwd: &Path, system: bool, allow_empty: bool) -> Result<SyncResult> {
    let config = if allow_empty {
        config::load_effective_or_empty(cwd)?
    } else {
        config::load_effective(cwd)?
    };

    if config.layers.is_empty() {
        return sync_empty(config);
    }

    let locations = StoreLocations::discover()?;
    store::sync(config, &locations, system)
}

fn sync_empty(config: EffectiveConfig) -> Result<SyncResult> {
    let locations = StoreLocations::discover()?;
    fs::create_dir_all(&locations.environments)?;
    let environment_dir = locations.environments.join("empty");
    let bin_dir = environment_dir.join("bin");
    fs::create_dir_all(&bin_dir)?;

    Ok(SyncResult {
        environment_dir,
        bin_dir,
        lock_path: PathBuf::new(),
        config,
        lock: store::LockFile {
            version: 1,
            target: platform::target_name(),
            environment_hash: "empty".to_owned(),
            packages: BTreeMap::new(),
        },
    })
}

fn print_sync_result(result: &SyncResult) {
    println!("environment: {}", result.environment_dir.display());
    println!("packages: {}", result.lock.packages.len());
    if !result.lock_path.as_os_str().is_empty() {
        println!("lockfile: {}", result.lock_path.display());
    }
}

fn shell(cwd: &Path, auto: bool, system: bool) -> Result<()> {
    if auto && env::var_os("PANMAN_DISABLE").is_some() {
        return Ok(());
    }
    if auto && env::var_os("PANMAN_SHELL_ACTIVE").is_some() {
        return Ok(());
    }

    let result = sync_current(cwd, system, auto)?;
    let program = result
        .config
        .shell_program
        .clone()
        .unwrap_or_else(platform::default_shell);

    let mut command = ProcessCommand::new(&program);
    apply_environment(&mut command, &result)?;
    command.env("PANMAN_SHELL_ACTIVE", "1");
    command.env("PANMAN_ENV_ROOT", &result.environment_dir);

    let status = command
        .status()
        .with_context(|| format!("failed to start shell {program:?}"))?;
    exit_with_status(status)
}

fn run(cwd: &Path, system: bool, arguments: Vec<OsString>) -> Result<()> {
    let result = sync_current(cwd, system, false)?;
    let status = if arguments.len() == 1 {
        let script_name = arguments[0].to_string_lossy();
        if let Some(script) = result.config.scripts.get(script_name.as_ref()) {
            run_script(script, &result)?
        } else {
            run_direct(arguments, &result)?
        }
    } else {
        run_direct(arguments, &result)?
    };

    exit_with_status(status)
}

fn run_direct(arguments: Vec<OsString>, result: &SyncResult) -> Result<ExitStatus> {
    let (program, args) = arguments
        .split_first()
        .context("run requires a command or script name")?;
    let mut command = ProcessCommand::new(program);
    command.args(args);
    apply_environment(&mut command, result)?;
    command.env("PANMAN_ENV_ROOT", &result.environment_dir);
    command
        .status()
        .with_context(|| format!("failed to execute {program:?}"))
}

fn run_script(script: &str, result: &SyncResult) -> Result<ExitStatus> {
    let mut command = if cfg!(windows) {
        let mut command = ProcessCommand::new("cmd.exe");
        command.args(["/D", "/S", "/C", script]);
        command
    } else {
        let shell = result
            .config
            .shell_program
            .as_deref()
            .unwrap_or("/bin/sh");
        let mut command = ProcessCommand::new(shell);
        command.args(["-lc", script]);
        command
    };
    apply_environment(&mut command, result)?;
    command.env("PANMAN_ENV_ROOT", &result.environment_dir);
    command
        .status()
        .with_context(|| format!("failed to execute script {script:?}"))
}

fn apply_environment(command: &mut ProcessCommand, result: &SyncResult) -> Result<()> {
    let mut paths = vec![result.bin_dir.clone()];
    if let Some(current) = env::var_os("PATH") {
        paths.extend(env::split_paths(&current));
    }
    command.env("PATH", env::join_paths(paths)?);
    command.envs(&result.config.env);
    Ok(())
}

fn exit_with_status(status: ExitStatus) -> Result<()> {
    if status.success() {
        Ok(())
    } else {
        let code = status.code().unwrap_or(1);
        std::process::exit(code)
    }
}

fn self_install() -> Result<PathBuf> {
    let source = env::current_exe().context("failed to locate the current Panman executable")?;
    let home = platform::panman_home()?;
    let bin = home.join("bin");
    fs::create_dir_all(&bin)?;
    let destination = bin.join(platform::executable_name());

    if source != destination {
        let temporary = bin.join(format!(".{}.tmp", platform::executable_name()));
        fs::copy(&source, &temporary).with_context(|| {
            format!(
                "failed to copy {} to {}",
                source.display(),
                temporary.display()
            )
        })?;
        #[cfg(windows)]
        if destination.exists() {
            fs::remove_file(&destination)?;
        }
        fs::rename(&temporary, &destination)?;
    }

    println!("installed {}", destination.display());
    Ok(destination)
}

fn enable_shell() -> Result<()> {
    let executable = self_install()?;
    let profile = platform::shell_profile()?;
    let existing = fs::read_to_string(&profile).unwrap_or_default();
    let cleaned = remove_managed_block(&existing);
    let block = shell_block(&executable);
    let mut updated = cleaned.trim_end().to_owned();
    if !updated.is_empty() {
        updated.push_str("\n\n");
    }
    updated.push_str(&block);
    updated.push('\n');
    write_atomic(&profile, updated.as_bytes())?;
    println!("enabled Panman shell startup in {}", profile.display());
    Ok(())
}

fn disable_shell() -> Result<()> {
    let profile = platform::shell_profile()?;
    if !profile.exists() {
        println!("Panman shell startup is already disabled");
        return Ok(());
    }

    let existing = fs::read_to_string(&profile)?;
    let updated = remove_managed_block(&existing);
    write_atomic(&profile, updated.as_bytes())?;
    println!("disabled Panman shell startup in {}", profile.display());
    Ok(())
}

#[cfg(unix)]
fn shell_block(executable: &Path) -> String {
    format!(
        "{SHELL_BLOCK_START}\nif [[ $- == *i* ]] && [[ -z \"${{PANMAN_SHELL_ACTIVE:-}}\" ]] && [[ -z \"${{PANMAN_DISABLE:-}}\" ]]; then\n    exec \"{}\" sh --auto\nfi\n{SHELL_BLOCK_END}",
        executable.display()
    )
}

#[cfg(windows)]
fn shell_block(executable: &Path) -> String {
    format!(
        "{SHELL_BLOCK_START}\nif (-not $env:PANMAN_SHELL_ACTIVE -and -not $env:PANMAN_DISABLE) {{\n    & \"{}\" sh --auto\n    exit $LASTEXITCODE\n}}\n{SHELL_BLOCK_END}",
        executable.display()
    )
}

fn remove_managed_block(contents: &str) -> String {
    let Some(start) = contents.find(SHELL_BLOCK_START) else {
        return contents.to_owned();
    };
    let Some(relative_end) = contents[start..].find(SHELL_BLOCK_END) else {
        return contents.to_owned();
    };
    let end = start + relative_end + SHELL_BLOCK_END.len();
    let mut output = String::with_capacity(contents.len());
    output.push_str(&contents[..start]);
    output.push_str(
        contents[end..].trim_start_matches(|character| matches!(character, '\r' | '\n')),
    );
    output
}

fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path.parent().context("target path has no parent")?;
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .context("target path has no file name")?
        .to_string_lossy();
    let temporary = parent.join(format!(".{file_name}.panman-tmp"));
    let mut file = fs::File::create(&temporary)?;
    file.write_all(contents)?;
    file.sync_all().ok();
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(&temporary, path)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_only_managed_shell_block() {
        let input = format!(
            "before\n{SHELL_BLOCK_START}\nmanaged\n{SHELL_BLOCK_END}\nafter\n"
        );
        assert_eq!(remove_managed_block(&input), "before\nafter\n");
    }
}
