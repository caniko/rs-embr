//! `embr index` — one-shot.

use std::path::Path;

use anyhow::Result;
use embr_core::{config::Config, pipeline::Pipeline};

pub async fn run(cfg: Config, state_dir: &Path) -> Result<()> {
    let db_path = state_dir.join("embr.sqlite");
    let pipeline = Pipeline::new(cfg, &db_path)?;
    let stats = pipeline.index_all().await?;
    let total_chunks: usize = stats.values().map(|s| s.chunks_upserted).sum();
    let total_errors: usize = stats.values().map(|s| s.errors).sum();
    tracing::info!(
        projects = stats.len(),
        total_chunks,
        total_errors,
        "index run complete"
    );
    if total_errors > 0 {
        std::process::exit(1);
    }
    Ok(())
}
