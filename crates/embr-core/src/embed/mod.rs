//! Embedding backends.

pub mod ollama;

pub use ollama::OllamaClient;

use async_trait::async_trait;

use crate::Result;

#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, model: &str, text: &str) -> Result<Vec<f32>>;
}
