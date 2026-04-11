//! End-to-end pipeline: walk → chunk → embed → upsert, with state
//! tracking for incremental re-runs and per-project orphan cleanup.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::chunker::{Chunker, LineChunker};
use crate::config::Config;
use crate::embed::{Embedder, OllamaClient};
use crate::state::StateStore;
use crate::store::qdrant::{Point, QdrantStore};
use crate::{Error, Result};

/// Stable namespace for point IDs — derived once from a fixed string.
/// UUID v5 over `(namespace, "project|path|chunk_index")` gives us
/// deterministic IDs across runs.
const POINT_NAMESPACE: Uuid = Uuid::from_u128(0x6f2f1c0e_4d7f_4b28_9e5a_0b1a0c3d2e1f);

/// Per-project indexing stats.
#[derive(Debug, Default, Clone)]
pub struct ProjectStats {
    pub files_total: usize,
    pub files_indexed: usize,
    pub files_skipped_unchanged: usize,
    pub files_skipped_too_large: usize,
    pub chunks_upserted: usize,
    pub orphans_removed: usize,
    pub errors: usize,
}

pub struct Pipeline {
    cfg: Config,
    chunker: Box<dyn Chunker>,
    embedder: Arc<dyn Embedder>,
    store: QdrantStore,
    state: StateStore,
}

impl Pipeline {
    pub fn new(cfg: Config, state_db_path: &Path) -> Result<Self> {
        let chunker: Box<dyn Chunker> =
            Box::new(LineChunker::new(cfg.chunking.max_lines, cfg.chunking.overlap));
        let embedder: Arc<dyn Embedder> = Arc::new(OllamaClient::new(&cfg.embedding.url));
        let store = QdrantStore::new(&cfg.qdrant.url, &cfg.qdrant.collection);
        let state = StateStore::open(state_db_path)?;
        Ok(Self {
            cfg,
            chunker,
            embedder,
            store,
            state,
        })
    }

    /// Bootstrap qdrant collection if absent.
    pub async fn ensure_collection(&self) -> Result<()> {
        let dim = self.cfg.vector_dim();
        let named: Vec<(String, usize)> = self
            .cfg
            .embedding
            .vectors
            .iter()
            .map(|v| (v.name.clone(), dim))
            .collect();
        self.store
            .ensure_collection(&named, &["project", "path", "hash"])
            .await
    }

    /// Index every configured project sequentially. This is the main entry
    /// point used by both `embr index` and `embr watch` on change events.
    pub async fn index_all(&self) -> Result<HashMap<String, ProjectStats>> {
        self.ensure_collection().await?;
        let mut all = HashMap::new();
        for project in &self.cfg.projects {
            match self.index_project(project).await {
                Ok(stats) => {
                    all.insert(project.clone(), stats);
                }
                Err(e) => {
                    tracing::error!(project, error = %e, "index_project failed");
                    let mut stats = ProjectStats::default();
                    stats.errors = 1;
                    all.insert(project.clone(), stats);
                }
            }
        }
        Ok(all)
    }

    /// Index a single project. Returns stats. Safe to call repeatedly; the
    /// state DB makes this cheap on a no-op.
    pub async fn index_project(&self, project: &str) -> Result<ProjectStats> {
        let t0 = std::time::Instant::now();
        let files = crate::walker::collect_source_files(
            &self.cfg.projects_root,
            project,
            &self.cfg.chunking.extensions,
        )?;
        let mut stats = ProjectStats::default();
        stats.files_total = files.len();

        let keep_paths: HashSet<String> =
            files.iter().map(|f| f.rel_path.clone()).collect();

        for file in &files {
            if let Err(e) = self.index_one_file(project, file, &mut stats).await {
                tracing::warn!(
                    project,
                    path = %file.rel_path,
                    error = %e,
                    "file failed, continuing"
                );
                stats.errors += 1;
            }
        }

        // Orphan sweep: anything in state/qdrant for this project that
        // isn't in `keep_paths` any more.
        match self.store.delete_orphans(project, &keep_paths).await {
            Ok(n) => stats.orphans_removed = n,
            Err(e) => {
                tracing::warn!(project, error = %e, "delete_orphans failed");
                stats.errors += 1;
            }
        }
        // Remove orphaned state entries so next run's hash-check is honest.
        for path in self.state.list_paths(project)? {
            if !keep_paths.contains(&path) {
                let _ = self.state.delete(project, &path);
            }
        }

        tracing::info!(
            project,
            files_total = stats.files_total,
            files_indexed = stats.files_indexed,
            files_skipped = stats.files_skipped_unchanged,
            chunks = stats.chunks_upserted,
            orphans = stats.orphans_removed,
            errors = stats.errors,
            elapsed_ms = t0.elapsed().as_millis(),
            "project indexed"
        );
        Ok(stats)
    }

    async fn index_one_file(
        &self,
        project: &str,
        file: &crate::walker::SourceFile,
        stats: &mut ProjectStats,
    ) -> Result<()> {
        let meta = match std::fs::metadata(&file.abs_path) {
            Ok(m) => m,
            Err(_) => return Ok(()),
        };
        if !meta.is_file() {
            return Ok(());
        }
        if meta.len() > self.cfg.chunking.max_file_bytes {
            stats.files_skipped_too_large += 1;
            return Ok(());
        }
        let content = match std::fs::read_to_string(&file.abs_path) {
            Ok(s) => s,
            Err(_) => return Ok(()), // binary / non-utf8: skip
        };
        if content.trim().is_empty() {
            return Ok(());
        }
        let hash = sha256_hex(&content);
        if let Some(existing) = self.state.get(project, &file.rel_path)? {
            if existing == hash {
                stats.files_skipped_unchanged += 1;
                return Ok(());
            }
        }

        // Drop stale points for this file before writing new ones.
        self.store.delete_by_file(project, &file.rel_path).await?;

        let chunks = self.chunker.chunk(&content);
        let mut points: Vec<Point> = Vec::with_capacity(chunks.len());
        for (i, chunk) in chunks.iter().enumerate() {
            if chunk.text.trim().is_empty() {
                continue;
            }
            let id = point_id(project, &file.rel_path, i);
            let mut vectors = HashMap::new();
            for nv in &self.cfg.embedding.vectors {
                let v = self
                    .embedder
                    .embed(&nv.model, &chunk.text)
                    .await
                    .map_err(|e| {
                        Error::Ollama(format!("{} chunk {}: {}", file.rel_path, i, e))
                    })?;
                if v.len() != nv.dim {
                    return Err(Error::Config(format!(
                        "model {} produced dim {} but config declares {}",
                        nv.model,
                        v.len(),
                        nv.dim
                    )));
                }
                vectors.insert(nv.name.clone(), v);
            }
            points.push(Point {
                id,
                vector: vectors,
                payload: serde_json::json!({
                    "project": project,
                    "path": file.rel_path,
                    "chunk": i,
                    "hash": hash,
                    "line_start": chunk.line_start,
                    "line_end": chunk.line_end,
                    "content": chunk.text,
                }),
            });
        }
        let n = points.len();
        self.store.upsert(&points).await?;
        self.state.put(project, &file.rel_path, &hash)?;
        stats.files_indexed += 1;
        stats.chunks_upserted += n;
        Ok(())
    }
}

fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    hex_encode(&digest)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn point_id(project: &str, rel_path: &str, chunk_index: usize) -> Uuid {
    let key = format!("{project}|{rel_path}|{chunk_index}");
    Uuid::new_v5(&POINT_NAMESPACE, key.as_bytes())
}
