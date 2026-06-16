//! Ollama `/api/embed` client.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::Embedder;
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct OllamaClient {
    base_url: String,
    http: reqwest::Client,
}

impl OllamaClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into();
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .expect("reqwest client build");
        Self { base_url, http }
    }
}

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a str,
}

#[derive(Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

#[async_trait]
impl Embedder for OllamaClient {
    async fn embed(&self, model: &str, text: &str) -> Result<Vec<f32>> {
        let url = format!("{}/api/embed", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(url)
            .json(&EmbedRequest { model, input: text })
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Ollama(format!("HTTP {status}: {body}")));
        }
        let parsed: EmbedResponse = resp.json().await?;
        parsed
            .embeddings
            .into_iter()
            .next()
            .ok_or_else(|| Error::Ollama("response contained no embeddings".into()))
    }
}
