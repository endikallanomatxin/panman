use std::path::PathBuf;

use anyhow::{Context, Result};

pub fn target_name() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

pub fn default_shell() -> String {
    if cfg!(windows) {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_owned())
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned())
    }
}

pub fn panman_home() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("PANMAN_HOME") {
        return Ok(PathBuf::from(path));
    }

    dirs::home_dir()
        .map(|home| home.join(".panman"))
        .context("could not determine the user's home directory")
}

pub fn system_home() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("PANMAN_SYSTEM_ROOT") {
        return Ok(PathBuf::from(path));
    }

    if cfg!(windows) {
        std::env::var_os("PROGRAMDATA")
            .map(PathBuf::from)
            .map(|path| path.join("Panman"))
            .context("could not determine PROGRAMDATA")
    } else {
        Ok(PathBuf::from("/.panman"))
    }
}

pub fn shell_profile() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not determine the user's home directory")?;
    if cfg!(windows) {
        Ok(home
            .join("Documents")
            .join("PowerShell")
            .join("Microsoft.PowerShell_profile.ps1"))
    } else {
        Ok(home.join(".bashrc"))
    }
}

pub fn executable_name() -> &'static str {
    if cfg!(windows) {
        "panman.exe"
    } else {
        "panman"
    }
}
