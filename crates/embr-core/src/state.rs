//! SQLite state store — tracks file content hashes so re-runs skip
//! unchanged files and only re-embed what moved.

use std::path::Path;

use rusqlite::{params, Connection};

use crate::Result;

pub struct StateStore {
    conn: Connection,
}

impl StateStore {
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS file_state (
                project TEXT NOT NULL,
                rel_path TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                indexed_at INTEGER NOT NULL,
                PRIMARY KEY (project, rel_path)
            )",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_file_state_project ON file_state(project)",
            [],
        )?;
        Ok(Self { conn })
    }

    /// Return the stored content hash for `(project, rel_path)`, if any.
    pub fn get(&self, project: &str, rel_path: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT content_hash FROM file_state WHERE project = ?1 AND rel_path = ?2")?;
        let mut rows = stmt.query(params![project, rel_path])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Upsert a file's hash.
    pub fn put(&self, project: &str, rel_path: &str, content_hash: &str) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn.execute(
            "INSERT INTO file_state (project, rel_path, content_hash, indexed_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(project, rel_path) DO UPDATE SET
                content_hash = excluded.content_hash,
                indexed_at = excluded.indexed_at",
            params![project, rel_path, content_hash, now],
        )?;
        Ok(())
    }

    /// Remove a file entry.
    pub fn delete(&self, project: &str, rel_path: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM file_state WHERE project = ?1 AND rel_path = ?2",
            params![project, rel_path],
        )?;
        Ok(())
    }

    /// Return all `rel_path` values the state DB knows about for a project.
    pub fn list_paths(&self, project: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT rel_path FROM file_state WHERE project = ?1")?;
        let rows = stmt
            .query_map(params![project], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Stats: number of tracked files for a project.
    pub fn count(&self, project: &str) -> Result<usize> {
        let n: i64 = self
            .conn
            .prepare_cached("SELECT COUNT(*) FROM file_state WHERE project = ?1")?
            .query_row(params![project], |r| r.get(0))?;
        Ok(n as usize)
    }
}
