//! Git-aware source file walker.
//!
//! We run `git ls-files --cached --others --exclude-standard` to get
//! tracked + non-ignored untracked files. This gives `.gitignore`
//! semantics for free and ignores the entire `.git` directory.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Error, Result};

/// A single source file inside a project.
#[derive(Debug, Clone)]
pub struct SourceFile {
    /// The project name (directory basename under projects_root).
    pub project: String,
    /// The path relative to the project root (as yielded by `git ls-files`).
    pub rel_path: String,
    /// Absolute path on disk.
    pub abs_path: PathBuf,
}

/// Enumerate tracked + non-ignored files in `project_path`. Returns relative
/// paths (same form `git ls-files` emits). An empty vec is returned for
/// non-git projects with a warning logged.
pub fn git_ls_files(project_path: &Path) -> Result<Vec<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(project_path)
        .arg("ls-files")
        .arg("--cached")
        .arg("--others")
        .arg("--exclude-standard")
        .output()
        .map_err(|e| Error::Git(format!("failed to spawn git: {e}")))?;

    if !out.status.success() {
        // Not fatal — some "projects" on disk may not be git repos.
        let stderr = String::from_utf8_lossy(&out.stderr);
        tracing::warn!(
            project = %project_path.display(),
            "git ls-files failed ({}): {}",
            out.status,
            stderr.trim()
        );
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    let files: Vec<String> = stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_owned())
        .collect();
    Ok(files)
}

/// Return `git rev-parse HEAD` for the project, or `None` if git fails.
/// Used in watch mode to detect new commits without re-walking the tree.
pub fn git_head(project_path: &Path) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(project_path)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Filter a raw git ls-files list down to source files by extension.
pub fn filter_by_extension<'a>(
    files: &'a [String],
    extensions: &std::collections::BTreeSet<String>,
) -> Vec<&'a String> {
    files
        .iter()
        .filter(|f| match Path::new(f).extension().and_then(|e| e.to_str()) {
            Some(ext) => extensions.contains(&ext.to_ascii_lowercase()),
            None => false,
        })
        .collect()
}

/// Build concrete SourceFile entries for a project.
pub fn collect_source_files(
    projects_root: &Path,
    project: &str,
    extensions: &std::collections::BTreeSet<String>,
) -> Result<Vec<SourceFile>> {
    let project_path = projects_root.join(project);
    if !project_path.exists() {
        tracing::warn!(project, path = %project_path.display(), "project directory does not exist, skipping");
        return Ok(Vec::new());
    }
    let files = git_ls_files(&project_path)?;
    let kept = filter_by_extension(&files, extensions);
    Ok(kept
        .into_iter()
        .map(|rel| SourceFile {
            project: project.to_owned(),
            rel_path: rel.clone(),
            abs_path: project_path.join(rel),
        })
        .collect())
}
