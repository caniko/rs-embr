//! Configuration schema (TOML-driven).
//!
//! The NixOS module generates a TOML file that matches this exactly.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub qdrant: QdrantConfig,
    pub embedding: EmbeddingConfig,
    /// Root directory containing project sub-directories.
    pub projects_root: PathBuf,
    /// Sub-directory names of projects to index under `projects_root`.
    #[serde(default)]
    pub projects: Vec<String>,
    #[serde(default)]
    pub chunking: ChunkingConfig,
    /// Where the SQLite state DB lives. When unset, the pipeline uses
    /// the caller-provided state dir (usually $STATE_DIRECTORY).
    #[serde(default)]
    pub state_dir: Option<PathBuf>,
    #[serde(default)]
    pub watch: WatchConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QdrantConfig {
    pub url: String,
    pub collection: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingConfig {
    #[serde(default)]
    pub backend: EmbeddingBackend,
    /// Base URL for the selected embedding API.
    pub url: String,
    /// Named vectors produced per chunk. Each entry becomes one named
    /// vector in the qdrant collection; every chunk is embedded by all of
    /// them. `name` is the qdrant vector name; `model` is the model
    /// identifier; `dim` is the vector dimension (must match the model's
    /// output).
    pub vectors: Vec<NamedVector>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingBackend {
    #[default]
    Ollama,
    Openai,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NamedVector {
    pub name: String,
    pub model: String,
    pub dim: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChunkingConfig {
    /// Maximum lines per chunk.
    #[serde(default = "default_max_lines")]
    pub max_lines: usize,
    /// Overlap between adjacent chunks, in lines.
    #[serde(default = "default_overlap")]
    pub overlap: usize,
    /// File extensions (without leading dot) considered source-code-like.
    #[serde(default = "default_extensions")]
    pub extensions: BTreeSet<String>,
    /// Skip files larger than this (bytes). Default 500 KB.
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: u64,
}

impl Default for ChunkingConfig {
    fn default() -> Self {
        Self {
            max_lines: default_max_lines(),
            overlap: default_overlap(),
            extensions: default_extensions(),
            max_file_bytes: default_max_file_bytes(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct WatchConfig {
    /// Interval between poll cycles, in seconds. Also used as the debounce
    /// window for local inotify events. Default 30s.
    #[serde(default = "default_watch_interval_secs")]
    pub interval_secs: u64,
    /// If true, force polling mode (needed for NFS — inotify doesn't
    /// propagate over NFS). When false, embr picks based on whether
    /// `projects_root` is an NFS mount.
    #[serde(default)]
    pub force_poll: bool,
}

fn default_max_lines() -> usize {
    80
}
fn default_overlap() -> usize {
    10
}
fn default_max_file_bytes() -> u64 {
    500_000
}
fn default_watch_interval_secs() -> u64 {
    30
}

fn default_extensions() -> BTreeSet<String> {
    [
        "nix", "rs", "ts", "tsx", "js", "jsx", "py", "go", "md", "txt", "toml", "yaml", "yml",
        "json", "sh", "nu", "html", "css", "scss", "sql", "proto", "graphql", "lua", "c", "h",
        "cpp", "hpp", "hs", "scala", "kt", "rb", "php", "swift", "zig", "elm", "ex", "exs", "erl",
        "ml", "mli",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect()
}

impl Config {
    pub fn from_path(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let cfg: Config = toml::from_str(&text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<()> {
        if self.embedding.vectors.is_empty() {
            return Err(Error::Config(
                "embedding.vectors must contain at least one named vector".into(),
            ));
        }
        // Enforce uniform dimension across named vectors: qdrant supports
        // per-vector dim, but keeping them equal simplifies dedup and
        // stops accidental mismatches. If you need mixed dims, relax this.
        let first_dim = self.embedding.vectors[0].dim;
        for nv in &self.embedding.vectors {
            if nv.dim != first_dim {
                return Err(Error::Config(format!(
                    "mixed dimensions not yet supported: {} has dim {}, first is {}",
                    nv.name, nv.dim, first_dim
                )));
            }
        }
        if self.chunking.max_lines == 0 {
            return Err(Error::Config("chunking.max_lines must be > 0".into()));
        }
        if self.chunking.overlap >= self.chunking.max_lines {
            return Err(Error::Config(
                "chunking.overlap must be < chunking.max_lines".into(),
            ));
        }
        Ok(())
    }

    /// Uniform dim across named vectors. Guaranteed non-zero after
    /// `validate()`.
    pub fn vector_dim(&self) -> usize {
        self.embedding.vectors[0].dim
    }

    pub fn query_vector(&self) -> &NamedVector {
        self.embedding
            .vectors
            .iter()
            .find(|vector| vector.name == "code")
            .unwrap_or(&self.embedding.vectors[0])
    }

    pub fn named_vector(&self, name: &str) -> Option<&NamedVector> {
        self.embedding.vectors.iter().find(|vector| vector.name == name)
    }
}
