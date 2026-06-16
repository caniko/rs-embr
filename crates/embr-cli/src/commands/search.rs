//! `embr search` — query the indexed code collection.

use std::path::Path;

use anyhow::Result;
use embr_core::{
    config::Config,
    search::{SearchRequest, SearchResponse, Searcher},
};

pub async fn run(
    cfg: Config,
    _state_dir: &Path,
    query: String,
    project: Option<String>,
    path_prefix: Option<String>,
    limit: usize,
) -> Result<()> {
    let searcher = Searcher::new(cfg);
    let results = searcher
        .search(SearchRequest {
            query,
            project,
            path_prefix,
            limit,
        })
        .await?;
    print!("{}", render_results(&results));
    Ok(())
}

pub fn render_results(results: &SearchResponse) -> String {
    if results.hits.is_empty() {
        return "No matches found.\n".into();
    }

    let mut out = String::new();
    for (index, hit) in results.hits.iter().enumerate() {
        let start = hit.line_start + 1;
        let end = hit.line_end;
        out.push_str(&format!(
            "[{rank}] score={score:.4} project={project} path={path} lines={start}-{end}\n",
            rank = index + 1,
            score = hit.score,
            project = hit.project,
            path = hit.path,
        ));
        for line in hit.content.lines() {
            out.push_str("    ");
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use embr_core::search::{SearchHit, SearchResponse};

    use super::render_results;

    #[test]
    fn render_results_includes_rank_score_path_and_content() {
        let rendered = render_results(&SearchResponse {
            hits: vec![SearchHit {
                project: "canix".into(),
                path: "root/hosts/nomad/infernis.nix".into(),
                line_start: 9,
                line_end: 18,
                score: 0.91234,
                content: "services.infernis.embr = {\n  enable = true;\n};".into(),
            }],
        });

        assert!(rendered.contains("[1] score=0.9123"));
        assert!(rendered.contains("project=canix"));
        assert!(rendered.contains("path=root/hosts/nomad/infernis.nix"));
        assert!(rendered.contains("lines=10-18"));
        assert!(rendered.contains("    services.infernis.embr = {"));
        assert!(rendered.contains("    enable = true;"));
    }
}
