use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "panman", version, about = "Declarative development environments")]
pub struct Cli {
    /// Resolve the environment as if Panman had been started in this directory.
    #[arg(long, global = true, value_name = "DIR")]
    pub cwd: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create .panman/panman.toml in the current directory.
    Init {
        #[arg(long)]
        force: bool,
    },

    /// Add a path or archive package to the nearest manifest.
    Add {
        name: String,

        #[arg(long, value_name = "DIR", conflicts_with = "url")]
        path: Option<PathBuf>,

        #[arg(long, value_name = "URL", conflicts_with = "path")]
        url: Option<String>,

        #[arg(long, requires = "url")]
        sha256: Option<String>,

        #[arg(long, default_value_t = 0, requires = "url")]
        strip_components: usize,

        #[arg(long = "bin", value_name = "PATH")]
        bins: Vec<PathBuf>,
    },

    /// Materialize packages and build the effective environment.
    Sync {
        /// Write missing packages to the system store instead of the user store.
        #[arg(long)]
        system: bool,
    },

    /// Start the configured shell inside the effective environment.
    Sh {
        /// Activation mode used by shell startup files. Falls back safely when no manifest exists.
        #[arg(long)]
        auto: bool,

        #[arg(long)]
        system: bool,
    },

    /// Run a command or a named manifest script inside the environment.
    Run {
        #[arg(long)]
        system: bool,

        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<OsString>,
    },

    /// Show discovered manifests and the composed environment.
    Status,

    /// Enable or disable automatic shell startup.
    Shell {
        #[command(subcommand)]
        command: ShellCommand,
    },

    /// Copy the current executable to the stable ~/.panman/bin location.
    SelfInstall,
}

#[derive(Debug, Subcommand)]
pub enum ShellCommand {
    Enable,
    Disable,
}
