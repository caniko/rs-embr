//! Qdrant HTTP client — just the operations embr needs.
//!
//! We deliberately skip the `qdrant-client` crate to keep the closure
//! small and the build simple (no gRPC / protobuf toolchain).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{Error, Result};

#[derive(Debug, Clone)]
pub struct QdrantStore {
    base_url: String,
    collection: String,
    http: reqwest::Client,
}

/// A point ready to upsert. `vectors` maps named vector → the dense vector.
#[derive(Debug, Clone, Serialize)]
pub struct Point {
    pub id: Uuid,
    pub vector: HashMap<String, Vec<f32>>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct SearchOptions<'a> {
    pub vector_name: &'a str,
    pub query_vector: &'a [f32],
    pub project: Option<&'a str>,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchPoint {
    pub score: f64,
    pub payload: SearchPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SearchPayload {
    pub project: String,
    pub path: String,
    pub line_start: usize,
    pub line_end: usize,
    pub content: String,
}

impl QdrantStore {
    pub fn new(base_url: impl Into<String>, collection: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("reqwest client build");
        Self {
            base_url: base_url.into(),
            collection: collection.into(),
            http,
        }
    }

    fn url(&self, tail: &str) -> String {
        format!(
            "{}/collections/{}{}",
            self.base_url.trim_end_matches('/'),
            self.collection,
            tail
        )
    }

    /// Create the collection if it does not exist, with the provided named
    /// vectors. Idempotent.
    pub async fn ensure_collection(
        &self,
        named_vectors: &[(String, usize)],
        payload_index_fields: &[&str],
    ) -> Result<()> {
        // Probe existence
        let probe = self.fetch_collection_info().await?;
        if let Some(body) = probe {
            validate_existing_vectors(&self.collection, named_vectors, &body)?;
            tracing::debug!(collection = %self.collection, "collection exists");
            return Ok(());
        }

        // Create
        tracing::info!(collection = %self.collection, "creating collection");
        let mut vectors = serde_json::Map::new();
        for (name, dim) in named_vectors {
            vectors.insert(
                name.clone(),
                json!({
                    "size": *dim,
                    "distance": "Cosine",
                }),
            );
        }
        let body = json!({ "vectors": serde_json::Value::Object(vectors) });
        let resp = self.http.put(self.url("")).json(&body).send().await?;
        if !resp.status().is_success() {
            return Err(Error::Qdrant(format!(
                "create failed: {} {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            )));
        }

        // Payload indexes (best-effort — ignore individual failures).
        for field in payload_index_fields {
            let _ = self
                .http
                .put(self.url("/index"))
                .json(&json!({
                    "field_name": field,
                    "field_schema": "keyword",
                }))
                .send()
                .await;
        }

        Ok(())
    }

    pub async fn validate_collection(&self, named_vectors: &[(String, usize)]) -> Result<()> {
        let Some(body) = self.fetch_collection_info().await? else {
            return Err(Error::Qdrant(format!(
                "collection {} does not exist; run indexing before searching",
                self.collection
            )));
        };

        validate_existing_vectors(&self.collection, named_vectors, &body)
    }

    /// Delete every point that matches `(project, path)`.
    pub async fn delete_by_file(&self, project: &str, rel_path: &str) -> Result<()> {
        let body = json!({
            "filter": {
                "must": [
                    { "key": "project", "match": { "value": project } },
                    { "key": "path", "match": { "value": rel_path } },
                ]
            }
        });
        let resp = self
            .http
            .post(self.url("/points/delete?wait=true"))
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(Error::Qdrant(format!(
                "delete_by_file: {} {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            )));
        }
        Ok(())
    }

    /// Delete every point for `project` whose `path` is not in `keep_paths`.
    /// Returns the number of deleted points.
    pub async fn delete_orphans(&self, project: &str, keep: &std::collections::HashSet<String>) -> Result<usize> {
        let mut total = 0usize;
        let mut offset: Option<serde_json::Value> = None;
        loop {
            let mut body = json!({
                "filter": {
                    "must": [
                        { "key": "project", "match": { "value": project } },
                    ]
                },
                "limit": 256,
                "with_payload": ["path"],
                "with_vector": false,
            });
            if let Some(off) = &offset {
                body.as_object_mut().unwrap().insert("offset".into(), off.clone());
            }
            let resp = self.http.post(self.url("/points/scroll")).json(&body).send().await?;
            if !resp.status().is_success() {
                return Err(Error::Qdrant(format!(
                    "scroll: {} {}",
                    resp.status(),
                    resp.text().await.unwrap_or_default()
                )));
            }
            #[derive(Deserialize)]
            struct ScrollResp {
                result: ScrollResult,
            }
            #[derive(Deserialize)]
            struct ScrollResult {
                points: Vec<ScrollPoint>,
                next_page_offset: Option<serde_json::Value>,
            }
            #[derive(Deserialize)]
            struct ScrollPoint {
                id: serde_json::Value,
                payload: ScrollPayload,
            }
            #[derive(Deserialize)]
            struct ScrollPayload {
                path: String,
            }
            let parsed: ScrollResp = resp.json().await?;
            let stale: Vec<serde_json::Value> = parsed
                .result
                .points
                .iter()
                .filter(|p| !keep.contains(&p.payload.path))
                .map(|p| p.id.clone())
                .collect();
            if !stale.is_empty() {
                let delete_body = json!({ "points": stale });
                let r = self
                    .http
                    .post(self.url("/points/delete?wait=true"))
                    .json(&delete_body)
                    .send()
                    .await?;
                if !r.status().is_success() {
                    return Err(Error::Qdrant(format!(
                        "delete orphans: {} {}",
                        r.status(),
                        r.text().await.unwrap_or_default()
                    )));
                }
                total += stale.len();
            }
            match parsed.result.next_page_offset {
                Some(o) => offset = Some(o),
                None => break,
            }
        }
        Ok(total)
    }

    /// Upsert a batch of points (atomic, waited).
    pub async fn upsert(&self, points: &[Point]) -> Result<()> {
        if points.is_empty() {
            return Ok(());
        }
        let body = json!({ "points": points });
        let resp = self
            .http
            .put(self.url("/points?wait=true"))
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(Error::Qdrant(format!(
                "upsert: {} {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            )));
        }
        Ok(())
    }

    pub async fn search(&self, options: &SearchOptions<'_>) -> Result<Vec<SearchPoint>> {
        #[derive(Deserialize)]
        struct QueryResponse {
            result: QueryResult,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum QueryResult {
            Points { points: Vec<SearchPoint> },
            Direct(Vec<SearchPoint>),
        }

        let mut body = json!({
            "query": options.query_vector,
            "using": options.vector_name,
            "limit": options.limit,
            "with_payload": ["project", "path", "line_start", "line_end", "content"],
            "with_vector": false,
        });

        if let Some(project) = options.project {
            body.as_object_mut().expect("query body is an object").insert(
                "filter".into(),
                json!({
                    "must": [
                        { "key": "project", "match": { "value": project } },
                    ]
                }),
            );
        }

        let resp = self
            .http
            .post(self.url("/points/query"))
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(Error::Qdrant(format!(
                "search: {} {}",
                resp.status(),
                resp.text().await.unwrap_or_default()
            )));
        }

        let parsed: QueryResponse = resp.json().await?;
        Ok(match parsed.result {
            QueryResult::Points { points } => points,
            QueryResult::Direct(points) => points,
        })
    }

    async fn fetch_collection_info(&self) -> Result<Option<String>> {
        let probe = self.http.get(self.url("")).send().await?;
        if probe.status() == reqwest::StatusCode::OK {
            return Ok(Some(probe.text().await.unwrap_or_default()));
        }
        if probe.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        Err(Error::Qdrant(format!(
            "probe failed: {} {}",
            probe.status(),
            probe.text().await.unwrap_or_default()
        )))
    }
}

fn validate_existing_vectors(
    collection: &str,
    expected_vectors: &[(String, usize)],
    body: &str,
) -> Result<()> {
    #[derive(Deserialize)]
    struct CollectionInfoResponse {
        result: CollectionInfo,
    }

    #[derive(Deserialize)]
    struct CollectionInfo {
        config: CollectionConfig,
    }

    #[derive(Deserialize)]
    struct CollectionConfig {
        params: CollectionParams,
    }

    #[derive(Deserialize)]
    struct CollectionParams {
        vectors: ExistingVectors,
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ExistingVectors {
        Named(HashMap<String, ExistingVector>),
        Single(ExistingVector),
    }

    #[derive(Debug, Deserialize)]
    struct ExistingVector {
        size: usize,
    }

    let parsed: CollectionInfoResponse = serde_json::from_str(body)?;
    let existing = match parsed.result.config.params.vectors {
        ExistingVectors::Named(named) => named
            .into_iter()
            .map(|(name, vector)| (name, vector.size))
            .collect::<HashMap<_, _>>(),
        ExistingVectors::Single(vector) => HashMap::from([("default".to_string(), vector.size)]),
    };
    let expected = expected_vectors
        .iter()
        .cloned()
        .collect::<HashMap<String, usize>>();

    if existing == expected {
        return Ok(());
    }

    Err(Error::Qdrant(format!(
        "collection {collection} exists with incompatible vectors; expected {expected:?}, found {existing:?}. delete the collection or switch to a new name before re-indexing"
    )))
}

#[cfg(test)]
mod tests {
    use mockito::{Matcher, Server};
    use serde_json::json;

    use super::{QdrantStore, SearchOptions};

    #[tokio::test]
    async fn search_posts_named_vector_query_and_parses_hits() {
        let mut server = Server::new_async().await;
        let _mock = server
            .mock("POST", "/collections/projects/points/query")
            .match_header("content-type", Matcher::Regex("application/json".into()))
            .match_body(Matcher::PartialJson(json!({
                "query": [0.25, 0.75],
                "using": "code",
                "limit": 3,
                "with_payload": ["project", "path", "line_start", "line_end", "content"],
                "with_vector": false,
                "filter": {
                    "must": [
                        { "key": "project", "match": { "value": "canix" } }
                    ]
                }
            })))
            .with_status(200)
            .with_body(
                json!({
                    "result": {
                        "points": [
                            {
                                "score": 0.91,
                                "payload": {
                                    "project": "canix",
                                    "path": "root/hosts/nomad/infernis.nix",
                                    "line_start": 12,
                                    "line_end": 24,
                                    "content": "services.infernis.embr = { ... };"
                                }
                            }
                        ]
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;

        let store = QdrantStore::new(server.url(), "projects");
        let hits = store
            .search(&SearchOptions {
                vector_name: "code",
                query_vector: &[0.25, 0.75],
                project: Some("canix"),
                limit: 3,
            })
            .await
            .expect("qdrant search should succeed");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].payload.project, "canix");
        assert_eq!(hits[0].payload.path, "root/hosts/nomad/infernis.nix");
        assert_eq!(hits[0].payload.line_start, 12);
        assert_eq!(hits[0].payload.line_end, 24);
        assert_eq!(hits[0].payload.content, "services.infernis.embr = { ... };");
        assert!((hits[0].score - 0.91).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn ensure_collection_rejects_incompatible_existing_schema() {
        let mut server = Server::new_async().await;
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
                                        "size": 768
                                    },
                                    "text": {
                                        "size": 768
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

        let store = QdrantStore::new(server.url(), "projects");
        let error = store
            .ensure_collection(&[("code".into(), 4096)], &[])
            .await
            .expect_err("schema mismatch should fail");

        assert!(error
            .to_string()
            .contains("collection projects exists with incompatible vectors"));
    }
}
