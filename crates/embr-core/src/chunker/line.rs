//! Line-window chunker — overlap-based sliding window.

use super::{Chunk, Chunker};

pub struct LineChunker {
    pub max_lines: usize,
    pub overlap: usize,
}

impl LineChunker {
    pub fn new(max_lines: usize, overlap: usize) -> Self {
        assert!(max_lines > 0);
        assert!(overlap < max_lines);
        Self { max_lines, overlap }
    }
}

impl Chunker for LineChunker {
    fn chunk(&self, content: &str) -> Vec<Chunk> {
        let lines: Vec<&str> = content.lines().collect();
        let n = lines.len();
        if n == 0 {
            return Vec::new();
        }
        if n <= self.max_lines {
            return vec![Chunk {
                line_start: 0,
                line_end: n,
                text: lines.join("\n"),
            }];
        }
        let step = self.max_lines.saturating_sub(self.overlap).max(1);
        let mut chunks = Vec::new();
        let mut i = 0usize;
        loop {
            let end = (i + self.max_lines).min(n);
            let text = lines[i..end].join("\n");
            chunks.push(Chunk {
                line_start: i,
                line_end: end,
                text,
            });
            if end == n {
                break;
            }
            i += step;
        }
        chunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty() {
        let c = LineChunker::new(10, 2);
        assert!(c.chunk("").is_empty());
    }

    #[test]
    fn under_max() {
        let c = LineChunker::new(10, 2);
        let out = c.chunk("a\nb\nc");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].line_start, 0);
        assert_eq!(out[0].line_end, 3);
    }

    #[test]
    fn sliding_window() {
        let c = LineChunker::new(4, 1);
        // 10 lines, window 4, step 3 → windows at 0,3,6,9; last clamps
        let content: String = (0..10).map(|i| format!("{i}\n")).collect();
        let out = c.chunk(&content);
        let ranges: Vec<(usize, usize)> = out.iter().map(|c| (c.line_start, c.line_end)).collect();
        assert_eq!(ranges, vec![(0, 4), (3, 7), (6, 10)]);
    }
}
