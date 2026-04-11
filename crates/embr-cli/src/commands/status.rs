//! `embr status` — show per-project counts from the local state DB.

use std::path::Path;

use anyhow::Result;
use embr_core::{config::Config, state::StateStore};

pub async fn run(cfg: Config, state_dir: &Path) -> Result<()> {
    let db_path = state_dir.join("embr.sqlite");
    let state = StateStore::open(&db_path)?;
    println!("state db: {}", db_path.display());
    println!();
    for project in &cfg.projects {
        let n = state.count(project).unwrap_or(0);
        println!("  {:<30} {:>6} files", project, n);
    }
    Ok(())
}
