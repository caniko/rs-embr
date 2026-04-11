//! embr — core library for declarative project code embedding indexing.
//!
//! The pipeline is:
//!
//! ```text
//! config -> walker -> chunker -> embedder (N named vectors) -> qdrant store
//!                           \-> state (sqlite: content hash)
//! ```
//!
//! Re-runs are idempotent: files whose content hash matches the state DB are
//! skipped entirely. Files no longer tracked by git are cleaned up per-project.

pub mod chunker;
pub mod config;
pub mod embed;
pub mod error;
pub mod pipeline;
pub mod state;
pub mod store;
pub mod walker;

pub use error::{Error, Result};
