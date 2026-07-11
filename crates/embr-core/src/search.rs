//! Query-time embedding + similarity search over the configured Qdrant code index.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use std::sync::Arc;

use crate::config::{Config, EmbeddingBackend};
use crate::embed::{Embedder, OllamaClient, OpenAiClient};
use crate::store::qdrant::{QdrantStore, SearchOptions};
use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct Searcher {
    cfg: Config,
    embedder: Arc<dyn Embedder>,
    store: QdrantStore,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub path_prefix: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SearchHit {
    pub project: String,
    pub path: String,
    pub line_start: usize,
    pub line_end: usize,
    pub score: f64,
    pub content: String,
}

impl Searcher {
    pub fn new(cfg: Config) -> Self {
        let embedder: Arc<dyn Embedder> = match cfg.embedding.backend {
            EmbeddingBackend::Ollama => Arc::new(OllamaClient::new(&cfg.embedding.url)),
            EmbeddingBackend::Openai => Arc::new(OpenAiClient::new(&cfg.embedding.url)),
        };
        let store = QdrantStore::new(&cfg.qdrant.url, &cfg.qdrant.collection);
        Self {
            cfg,
            embedder,
            store,
        }
    }

    pub async fn search(&self, request: SearchRequest) -> Result<SearchResponse> {
        let query = request.query.trim();
        if query.is_empty() {
            return Err(Error::Config("search query must not be empty".into()));
        }
        if request.limit == 0 {
            return Err(Error::Config("search limit must be > 0".into()));
        }

        let vector = self.cfg.query_vector();
        self.store
            .validate_collection(&[(vector.name.clone(), vector.dim)])
            .await?;
        let embedded = self.embedder.embed(&vector.model, query).await?;
        if embedded.len() != vector.dim {
            return Err(Error::Config(format!(
                "model {} produced dim {} but config declares {}",
                vector.model,
                embedded.len(),
                vector.dim
            )));
        }

        let fetch_limit = overfetch_limit(request.limit, request.path_prefix.as_deref());
        let path_prefix = request.path_prefix.as_deref();
        let points = self
            .store
            .search(&SearchOptions {
                vector_name: &vector.name,
                query_vector: &embedded,
                project: request.project.as_deref(),
                limit: fetch_limit,
            })
            .await?;

        let hits = points
            .into_iter()
            .filter(|point| {
                path_prefix
                    .map(|prefix| point.payload.path.starts_with(prefix))
                    .unwrap_or(true)
            })
            .take(request.limit)
            .map(|point| SearchHit {
                project: point.payload.project,
                path: point.payload.path,
                line_start: point.payload.line_start,
                line_end: point.payload.line_end,
                score: point.score,
                content: point.payload.content,
            })
            .collect();

        Ok(SearchResponse { hits })
    }
}

fn default_limit() -> usize {
    10
}

fn overfetch_limit(limit: usize, path_prefix: Option<&str>) -> usize {
    if path_prefix.is_some() {
        limit.saturating_mul(5).clamp(limit, 200)
    } else {
        limit
    }
}

#[cfg(test)]
mod tests {
    use mockito::{Matcher, Server};
    use serde_json::json;
    use std::path::PathBuf;

    use super::{SearchRequest, Searcher};
    use crate::config::{
        Config, EmbeddingBackend, EmbeddingConfig, NamedVector, QdrantConfig, WatchConfig,
    };

    fn test_config(base_url: &str) -> Config {
        Config {
            qdrant: QdrantConfig {
                url: base_url.to_string(),
                collection: "projects".into(),
            },
            embedding: EmbeddingConfig {
                backend: EmbeddingBackend::Ollama,
                url: base_url.to_string(),
                vectors: vec![
                    NamedVector {
                        name: "docs".into(),
                        model: "qwen3-embedding:4b".into(),
                        dim: 2,
                    },
                    NamedVector {
                        name: "code".into(),
                        model: "qwen3-embedding:8b".into(),
                        dim: 2,
                    },
                ],
            },
            projects_root: PathBuf::from("/var/empty"),
            projects: vec![],
            chunking: Default::default(),
            state_dir: None,
            watch: WatchConfig::default(),
        }
    }

    #[tokio::test]
    async fn search_prefers_code_vector_model_and_filters_path_prefix_client_side() {
        let mut server = Server::new_async().await;
        let _embed = server
            .mock("POST", "/api/embed")
            .match_body(Matcher::PartialJson(json!({
                "model": "qwen3-embedding:8b",
                "input": "infernis wrapper"
            })))
            .with_status(200)
            .with_body(json!({ "embeddings": [[0.1, 0.2]] }).to_string())
            .create_async()
            .await;
        let _probe = server
            .mock("GET", "/collections/projects")
            .with_status(200)
            .with_body(
                json!({
                    "result": {
                        "config": {
                            "params": {
                                "vectors": {
                                    "code": {
                                        "size": 2
                                    }
                                }
                            }
                        }
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;
        let _query = server
            .mock("POST", "/collections/projects/points/query")
            .with_status(200)
            .with_body(
                json!({
                    "result": {
                        "points": [
                            {
                                "score": 0.95,
                                "payload": {
                                    "project": "canix",
                                    "path": "root/hosts/nomad/infernis.nix",
                                    "line_start": 10,
                                    "line_end": 18,
                                    "content": "services.infernis.embr = { ... };"
                                }
                            },
                            {
                                "score": 0.92,
                                "payload": {
                                    "project": "canix",
                                    "path": "README.md",
                                    "line_start": 1,
                                    "line_end": 3,
                                    "content": "ignore me"
                                }
                            }
                        ]
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let searcher = Searcher::new(test_config(&server.url()));
        let result = searcher
            .search(SearchRequest {
                query: "infernis wrapper".into(),
                project: Some("canix".into()),
                path_prefix: Some("root/".into()),
                limit: 2,
            })
            .await
            .expect("search should succeed");

        assert_eq!(result.hits.len(), 1);
        assert_eq!(result.hits[0].project, "canix");
        assert_eq!(result.hits[0].path, "root/hosts/nomad/infernis.nix");
        assert_eq!(result.hits[0].line_start, 10);
        assert_eq!(result.hits[0].line_end, 18);
        assert_eq!(result.hits[0].content, "services.infernis.embr = { ... };");
    }
}
