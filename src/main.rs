//! Deploys binaries to plain Linux hosts over ssh and runs them under systemd.

mod cli;
mod config;
mod deploy;
mod release;
mod secrets;
mod ssh;
mod ui;
mod unit;

use std::path::Path;

use anyhow::{Result, bail};
use clap::Parser;

use crate::cli::{Cli, Command, SecretsAction};
use crate::config::Config;
use crate::deploy::Opts;

fn main() {
    if let Err(err) = run() {
        ui::fail(&format!("error: {err:#}"));
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Init => init(),
        Command::Deploy {
            common,
            dry_run,
            skip_build,
        } => {
            let config = Config::load(&common.config)?;
            deploy::deploy(
                &config,
                &Opts {
                    dry_run,
                    skip_build,
                    host_filter: common.host,
                    group_filter: common.group,
                },
            )
        }
        Command::Rollback { common } => {
            let config = Config::load(&common.config)?;
            deploy::rollback(
                &config,
                &Opts {
                    dry_run: false,
                    skip_build: true,
                    host_filter: common.host,
                    group_filter: common.group,
                },
            )
        }
        Command::Status { common } => {
            let config = Config::load(&common.config)?;
            deploy::status(
                &config,
                &Opts {
                    dry_run: false,
                    skip_build: true,
                    host_filter: common.host,
                    group_filter: common.group,
                },
            )
        }
        Command::Secrets { action } => match action {
            SecretsAction::Init { config } => secrets::init(&Config::load(&config)?),
            SecretsAction::Edit { config } => secrets::edit(&Config::load(&config)?),
            SecretsAction::Push { common, dry_run } => {
                let config = Config::load(&common.config)?;
                deploy::secrets_push(
                    &config,
                    &Opts {
                        dry_run,
                        skip_build: true,
                        host_filter: common.host,
                        group_filter: common.group,
                    },
                )
            }
        },
    }
}

fn init() -> Result<()> {
    let path = Path::new("slipway.toml");
    if path.exists() {
        bail!("slipway.toml already exists here");
    }
    std::fs::write(path, config::EXAMPLE)?;
    ui::ok("wrote slipway.toml");
    Ok(())
}
