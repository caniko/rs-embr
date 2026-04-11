//! Qdrant HTTP client — just the operations embr needs.
//!
//! We deliberately skip the `qdrant-client` crate to keep the closure
//! small and the build simple (no gRPC / protobuf toolchain).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{Error, Result};

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
        let probe = self
            .http
            .get(self.url(""))
            .send()
            .await?;
        if probe.status() == reqwest::StatusCode::OK {
            tracing::debug!(collection = %self.collection, "collection exists");
            return Ok(());
        }
        if probe.status() != reqwest::StatusCode::NOT_FOUND {
            return Err(Error::Qdrant(format!(
                "probe failed: {} {}",
                probe.status(),
                probe.text().await.unwrap_or_default()
            )));
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
}
