//! OpenAI-compatible /v1/embeddings client.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::Embedder;
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct OpenAiClient {
    base_url: String,
    http: reqwest::Client,
}

impl OpenAiClient {
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
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

#[async_trait]
impl Embedder for OpenAiClient {
    async fn embed(&self, model: &str, text: &str) -> Result<Vec<f32>> {
        let base = self.base_url.trim_end_matches('/');
        let url = if base.ends_with("/v1") {
            format!("{base}/embeddings")
        } else {
            format!("{base}/v1/embeddings")
        };
        let resp = self
            .http
            .post(url)
            .json(&EmbedRequest { model, input: text })
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::OpenAi(format!("HTTP {status}: {body}")));
        }
        let parsed: EmbedResponse = resp.json().await?;
        parsed
            .data
            .into_iter()
            .next()
            .map(|entry| entry.embedding)
            .ok_or_else(|| Error::OpenAi("response contained no embeddings".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::OpenAiClient;
    use crate::embed::Embedder;
    use mockito::Server;

    #[tokio::test]
    async fn posts_openai_embedding_request() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/embeddings")
            .match_header("content-type", "application/json")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "model": "qwen3-embedding-8b",
                "input": "hello"
            })))
            .with_status(200)
            .with_body(r#"{"data":[{"embedding":[0.1,0.2]}]}"#)
            .create_async()
            .await;

        let client = OpenAiClient::new(server.url());
        let embedding = client
            .embed("qwen3-embedding-8b", "hello")
            .await
            .expect("OpenAI-compatible embedding request should succeed");

        assert_eq!(embedding, vec![0.1, 0.2]);
        mock.assert_async().await;
    }
}
