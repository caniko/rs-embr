//! `embr watch` — long-running daemon that re-indexes on change.
//!
//! v1 strategy: purely poll-based. Every `watch.interval_secs` we check
//! each project's `git rev-parse HEAD` and the mtime of its working tree.
//! If either changed since the last cycle, we re-index that project and
//! nothing else. This handles both local FS and NFS uniformly — inotify
//! doesn't propagate over NFS anyway.
//!
//! Incremental hashing in the pipeline makes per-project re-runs cheap
//! when only a few files changed.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use embr_core::{config::Config, pipeline::Pipeline, walker};

pub async fn run(cfg: Config, state_dir: &Path) -> Result<()> {
    let db_path = state_dir.join("embr.sqlite");
    let interval = Duration::from_secs(cfg.watch.interval_secs.max(5));
    let pipeline = Pipeline::new(cfg.clone(), &db_path)?;
    pipeline.ensure_collection().await?;

    // Initial full pass — every project, unconditional. This also
    // populates the fingerprint map used below.
    tracing::info!("initial full index pass");
    let _ = pipeline.index_all().await?;

    let mut fingerprints: HashMap<String, ProjectFingerprint> = HashMap::new();
    for project in &cfg.projects {
        let fp = fingerprint(&cfg.projects_root.join(project));
        fingerprints.insert(project.clone(), fp);
    }
    tracing::info!(interval_secs = interval.as_secs(), "watch loop started");

    let mut ticker = tokio::time::interval(interval);
    // Eat the immediate first tick — we just did an index_all.
    ticker.tick().await;

    // SIGTERM / Ctrl-C graceful shutdown
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("ctrl-c received, exiting");
                return Ok(());
            }
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM received, exiting");
                return Ok(());
            }
        }

        for project in &cfg.projects {
            let project_path = cfg.projects_root.join(project);
            let current = fingerprint(&project_path);
            let prev = fingerprints.get(project).cloned().unwrap_or_default();
            if current == prev {
                continue;
            }
            tracing::info!(project, "change detected, re-indexing");
            match pipeline.index_project(project).await {
                Ok(stats) => {
                    tracing::debug!(project, ?stats, "re-index complete");
                    fingerprints.insert(project.clone(), current);
                }
                Err(e) => {
                    tracing::warn!(project, error = %e, "re-index failed, will retry");
                    // Leave fingerprint unchanged so we retry next cycle.
                }
            }
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ProjectFingerprint {
    head: Option<String>,
    /// Unix seconds of the most recent mtime among tracked files.
    latest_mtime: i64,
}

/// Cheap-ish project fingerprint: git HEAD + max file mtime across the
/// working tree. Avoids hashing content — the pipeline already does that.
fn fingerprint(project_path: &Path) -> ProjectFingerprint {
    let head = walker::git_head(project_path);
    let latest_mtime = latest_mtime_via_git(project_path);
    ProjectFingerprint { head, latest_mtime }
}

/// Max mtime (seconds since UNIX epoch) across files tracked by git.
/// Returns 0 on any failure.
fn latest_mtime_via_git(project_path: &Path) -> i64 {
    let Ok(files) = walker::git_ls_files(project_path) else {
        return 0;
    };
    let mut best = 0i64;
    for rel in &files {
        let p = project_path.join(rel);
        if let Ok(md) = std::fs::metadata(&p) {
            if let Ok(mtime) = md.modified() {
                if let Ok(d) = mtime.duration_since(std::time::UNIX_EPOCH) {
                    let secs = d.as_secs() as i64;
                    if secs > best {
                        best = secs;
                    }
                }
            }
        }
    }
    best
}
