//! Command-line surface.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "slipway",
    version,
    about = "Deploys binaries to plain Linux hosts over ssh"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Write an example slipway.toml into the current directory
    Init,
    /// Build the artifact and roll it out to every host
    Deploy {
        #[command(flatten)]
        common: Common,
        /// Print every command instead of executing anything
        #[arg(long)]
        dry_run: bool,
        /// Deploy the existing artifact without running the build command
        #[arg(long)]
        skip_build: bool,
    },
    /// Flip hosts back to the release preceding the current one
    Rollback {
        #[command(flatten)]
        common: Common,
    },
    /// Show current release, service state, and recent releases per host
    Status {
        #[command(flatten)]
        common: Common,
    },
    /// Manage the age-encrypted secrets file
    Secrets {
        #[command(subcommand)]
        action: SecretsAction,
    },
}

#[derive(Subcommand)]
pub enum SecretsAction {
    /// Create the local identity, then the encrypted secrets file
    Init {
        /// Path to the config file
        #[arg(short, long, default_value = "slipway.toml")]
        config: PathBuf,
    },
    /// Decrypt into $EDITOR, validate, re-encrypt to every recipient
    Edit {
        /// Path to the config file
        #[arg(short, long, default_value = "slipway.toml")]
        config: PathBuf,
    },
    /// Upload the current secrets to the hosts, restart, health-check
    Push {
        #[command(flatten)]
        common: Common,
        /// Print every command instead of executing anything
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Args)]
pub struct Common {
    /// Path to the config file
    #[arg(short, long, default_value = "slipway.toml")]
    pub config: PathBuf,
    /// Only hosts whose ssh destination contains this substring
    #[arg(long)]
    pub host: Option<String>,
    /// Only hosts labeled with this group in the config
    #[arg(long)]
    pub group: Option<String>,
}
