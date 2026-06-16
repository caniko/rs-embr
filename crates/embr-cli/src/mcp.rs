//! `embr mcp` — stdio MCP server exposing semantic code search.

use anyhow::Result;
use embr_core::{
    config::Config,
    search::{SearchRequest, SearchResponse, Searcher},
};
use rmcp::{
    Json, ServiceExt,
    handler::server::wrapper::Parameters,
    schemars, tool, tool_router,
};

#[derive(Debug, Clone)]
pub struct EmbrMcpServer {
    searcher: Searcher,
}

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
pub struct EmbrSearchCodeRequest {
    pub query: String,
    #[schemars(description = "Optional exact project name to filter by.")]
    pub project: Option<String>,
    #[schemars(description = "Optional path prefix to filter by after similarity search.")]
    pub path_prefix: Option<String>,
    #[schemars(description = "Maximum number of hits to return. Defaults to 10.")]
    pub limit: Option<usize>,
}

impl EmbrMcpServer {
    pub fn new(cfg: Config) -> Self {
        Self {
            searcher: Searcher::new(cfg),
        }
    }
}

#[tool_router(server_handler)]
impl EmbrMcpServer {
    #[tool(
        description = "Semantic code search across the indexed Embr projects collection."
    )]
    pub async fn embr_search_code(
        &self,
        Parameters(request): Parameters<EmbrSearchCodeRequest>,
    ) -> Result<Json<SearchResponse>, String> {
        let result = self
            .searcher
            .search(SearchRequest {
                query: request.query,
                project: request.project,
                path_prefix: request.path_prefix,
                limit: request.limit.unwrap_or(10),
            })
            .await
            .map_err(|error| error.to_string())?;
        Ok(Json(result))
    }
}

pub async fn run(cfg: Config) -> Result<()> {
    EmbrMcpServer::new(cfg)
        .serve(rmcp::transport::stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use embr_core::config::{Config, EmbeddingConfig, NamedVector, QdrantConfig, WatchConfig};
    use mockito::{Matcher, Server};
    use serde_json::json;
    use std::path::PathBuf;

    use super::{EmbrMcpServer, EmbrSearchCodeRequest};

    fn test_config(base_url: &str) -> Config {
        Config {
            qdrant: QdrantConfig {
                url: base_url.to_string(),
                collection: "projects".into(),
            },
            embedding: EmbeddingConfig {
                url: base_url.to_string(),
                vectors: vec![NamedVector {
                    name: "code".into(),
                    model: "qwen3-embedding:8b".into(),
                    dim: 2,
                }],
            },
            projects_root: PathBuf::from("/var/empty"),
            projects: vec![],
            chunking: Default::default(),
            state_dir: None,
            watch: WatchConfig::default(),
        }
    }

    #[tokio::test]
    async fn embr_search_code_tool_returns_structured_hits() {
        let mut server = Server::new_async().await;
        let _embed = server
            .mock("POST", "/api/embed")
            .match_body(Matcher::PartialJson(json!({
                "model": "qwen3-embedding:8b",
                "input": "qdrant wrapper"
            })))
            .with_status(200)
            .with_body(json!({ "embeddings": [[0.4, 0.6]] }).to_string())
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
                                "score": 0.88,
                                "payload": {
                                    "project": "canix",
                                    "path": "home/hosts/atlas/yeehaw.nix",
                                    "line_start": 3,
                                    "line_end": 7,
                                    "content": "programs.yh.mcpServers.embr = { ... };"
                                }
                            }
                        ]
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let server = EmbrMcpServer::new(test_config(&server.url()));
        let result = server
            .embr_search_code(rmcp::handler::server::wrapper::Parameters(
                EmbrSearchCodeRequest {
                    query: "qdrant wrapper".into(),
                    project: Some("canix".into()),
                    path_prefix: None,
                    limit: Some(5),
                },
            ))
            .await
            .expect("tool should succeed");

        let structured = serde_json::to_value(result.0).expect("tool response should serialize");
        assert_eq!(structured["hits"][0]["project"], "canix");
        assert_eq!(structured["hits"][0]["path"], "home/hosts/atlas/yeehaw.nix");
        assert_eq!(structured["hits"][0]["line_start"], 3);
        assert_eq!(structured["hits"][0]["line_end"], 7);
        assert_eq!(structured["hits"][0]["score"], 0.88);
        assert_eq!(
            structured["hits"][0]["content"],
            "programs.yh.mcpServers.embr = { ... };"
        );
    }
}
