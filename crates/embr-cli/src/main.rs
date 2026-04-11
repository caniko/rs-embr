//! embr — declarative project code embedding indexer.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

mod commands;

#[derive(Parser)]
#[command(
    name = "embr",
    version,
    about = "Declarative project code embedding indexer"
)]
struct Cli {
    /// Path to the TOML config file.
    #[arg(short, long, env = "EMBR_CONFIG", global = true)]
    config: Option<PathBuf>,

    /// Override the state directory (SQLite DB + runtime data).
    #[arg(long, env = "EMBR_STATE_DIR", global = true)]
    state_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run one full indexing pass and exit.
    Index,
    /// Watch configured projects and re-index on change. Long-running.
    Watch,
    /// Print per-project state DB statistics.
    Status,
    /// Delete the state DB and exit. Does NOT touch qdrant.
    Reset,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,embr=debug")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    let config_path = cli
        .config
        .clone()
        .context("--config or EMBR_CONFIG is required")?;
    let cfg = embr_core::config::Config::from_path(&config_path)
        .with_context(|| format!("loading {}", config_path.display()))?;
    let state_dir = cli
        .state_dir
        .or_else(|| cfg.state_dir.clone())
        .unwrap_or_else(default_state_dir);

    match cli.command {
        Command::Index => commands::index::run(cfg, &state_dir).await,
        Command::Watch => commands::watch::run(cfg, &state_dir).await,
        Command::Status => commands::status::run(cfg, &state_dir).await,
        Command::Reset => commands::reset::run(&state_dir).await,
    }
}

fn default_state_dir() -> PathBuf {
    if let Ok(sd) = std::env::var("STATE_DIRECTORY") {
        // systemd sets this.
        return PathBuf::from(sd);
    }
    if let Some(xdg) = dirs_state_home() {
        return xdg.join("embr");
    }
    PathBuf::from("/var/lib/embr")
}

fn dirs_state_home() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("XDG_STATE_HOME") {
        return Some(PathBuf::from(p));
    }
    if let Ok(home) = std::env::var("HOME") {
        return Some(PathBuf::from(home).join(".local/state"));
    }
    None
}
