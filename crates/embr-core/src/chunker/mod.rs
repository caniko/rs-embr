//! Chunking — split file content into overlapping windows.
//!
//! v1 uses a simple line-based chunker: fixed-size window with fixed
//! overlap. Future AST-aware chunkers (tree-sitter) will live as
//! additional modules behind the same `Chunker` trait.

pub mod line;

pub use line::LineChunker;

/// One chunk of a source file.
#[derive(Debug, Clone)]
pub struct Chunk {
    /// Zero-based inclusive.
    pub line_start: usize,
    /// Zero-based exclusive (i.e. Python-slice style).
    pub line_end: usize,
    pub text: String,
}

pub trait Chunker: Send + Sync {
    fn chunk(&self, content: &str) -> Vec<Chunk>;
}
