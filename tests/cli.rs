use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

fn panman(home: &Path) -> Command {
    let mut command = Command::cargo_bin("panman").unwrap();
    command.env("PANMAN_HOME", home.join("panman-home"));
    command.env("PANMAN_SYSTEM_ROOT", home.join("system-panman"));
    command
}

#[test]
fn init_creates_a_manifest() {
    let temp = tempdir().unwrap();
    panman(temp.path())
        .arg("--cwd")
        .arg(temp.path())
        .arg("init")
        .assert()
        .success();

    let manifest = temp.path().join(".panman/panman.toml");
    assert!(manifest.is_file());
    assert!(fs::read_to_string(manifest)
        .unwrap()
        .contains("[environment]"));
}

#[test]
fn status_composes_parent_and_child_manifests() {
    let temp = tempdir().unwrap();
    let child = temp.path().join("project");
    fs::create_dir_all(temp.path().join(".panman")).unwrap();
    fs::create_dir_all(child.join(".panman")).unwrap();
    fs::write(
        temp.path().join(".panman/panman.toml"),
        "[env]\nFROM_PARENT = 'yes'\n",
    )
    .unwrap();
    fs::write(
        child.join(".panman/panman.toml"),
        "[scripts]\ntest = 'echo test'\n",
    )
    .unwrap();

    panman(temp.path())
        .arg("--cwd")
        .arg(&child)
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("environment variables: 1"))
        .stdout(predicate::str::contains("scripts: 1"));
}

#[test]
fn sync_imports_a_local_package_and_writes_a_lockfile() {
    let temp = tempdir().unwrap();
    let package = temp.path().join("tool");
    let bin = package.join("bin");
    fs::create_dir_all(&bin).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let executable = bin.join("hello");
        fs::write(&executable, "#!/bin/sh\necho hello\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(windows)]
    fs::write(bin.join("hello.cmd"), "@echo hello\r\n").unwrap();

    fs::create_dir_all(temp.path().join(".panman")).unwrap();
    let executable = if cfg!(windows) {
        "bin/hello.cmd"
    } else {
        "bin/hello"
    };
    fs::write(
        temp.path().join(".panman/panman.toml"),
        format!(
            "[packages.hello]\nsource = 'path'\npath = './tool'\nbins = ['{executable}']\n"
        ),
    )
    .unwrap();

    panman(temp.path())
        .arg("--cwd")
        .arg(temp.path())
        .arg("sync")
        .assert()
        .success()
        .stdout(predicate::str::contains("packages: 1"));

    assert!(temp.path().join(".panman/panman.lock").is_file());
    assert!(temp
        .path()
        .join("panman-home/store")
        .read_dir()
        .unwrap()
        .any(|entry| entry.unwrap().file_name().to_string_lossy().contains("hello")));
}
