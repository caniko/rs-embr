//! `embr reset` — delete the state DB. Does NOT touch qdrant.

use std::path::Path;

use anyhow::Result;

pub async fn run(state_dir: &Path) -> Result<()> {
    let db_path = state_dir.join("embr.sqlite");
    for suffix in ["", "-wal", "-shm"] {
        let p = state_dir.join(format!("embr.sqlite{suffix}"));
        if p.exists() {
            std::fs::remove_file(&p)?;
            tracing::info!(path = %p.display(), "removed");
        }
    }
    let _ = db_path;
    Ok(())
}
