//! Byte-preserving document models and gix text alignment for both panels.
use gix::diff::blob::{Algorithm, InternedInput, diff_with_slider_heuristics, sources::lines};
use std::{ops::Range, path::PathBuf};

pub const MAX_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_LINES: usize = 40_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    Unstaged,
    Staged,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    pub bytes: Vec<u8>,
    pub present: bool,
    pub mode: Option<u32>,
    pub size: u64,
    pub limited: bool,
    pub unsupported: bool,
}

impl Document {
    /// Constructs the empty version of an absent file, distinct from an empty file.
    pub fn absent() -> Self {
        Self {
            bytes: Vec::new(),
            present: false,
            mode: None,
            size: 0,
            limited: false,
            unsupported: false,
        }
    }
    /// Retains original bytes and mode, without newline or encoding transformations.
    pub fn present(bytes: Vec<u8>, mode: Option<u32>) -> Self {
        Self {
            size: bytes.len() as u64,
            bytes,
            present: true,
            mode,
            limited: false,
            unsupported: false,
        }
    }
    /// Records large content without allocating or decompressing the entire blob.
    pub fn oversized(size: u64, mode: Option<u32>) -> Self {
        Self {
            size,
            mode,
            present: true,
            limited: true,
            ..Self::absent()
        }
    }
    /// Records modes such as gitlinks without presenting them as empty text files.
    pub fn unsupported(mode: Option<u32>) -> Self {
        Self {
            mode,
            present: true,
            unsupported: true,
            ..Self::absent()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presentation {
    Text,
    Binary,
    InvalidEncoding,
    TooLarge,
    Conflict,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub number: usize,
    pub text: String,
    pub ending: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffRow {
    pub left: Option<Line>,
    pub right: Option<Line>,
    pub changed: bool,
}

#[derive(Clone, Debug)]
pub struct DiffSnapshot {
    pub path: PathBuf,
    pub kind: DiffKind,
    pub left: Document,
    pub right: Document,
    pub presentation: Presentation,
    pub rows: Vec<DiffRow>,
    pub hunks: Vec<Range<usize>>,
    pub backend: &'static str,
    pub(crate) guard: Option<crate::repository::RepositoryGuard>,
}

/// Separates display text from its exact newline representation.
fn line(text: &str, index: usize) -> Line {
    let (text, ending) = if let Some(text) = text.strip_suffix("\r\n") {
        (text, "CRLF")
    } else if let Some(text) = text.strip_suffix('\n') {
        (text, "LF")
    } else {
        (text, "EOF")
    };
    Line {
        number: index + 1,
        text: text.to_owned(),
        ending,
    }
}

impl DiffSnapshot {
    /// Compares exact source versions as well as bytes; a new index needs a new write guard.
    pub fn same_version(&self, other: &Self) -> bool {
        self.path == other.path
            && self.kind == other.kind
            && self.left == other.left
            && self.right == other.right
            && self.presentation == other.presentation
            && self.guard == other.guard
    }

    /// Constructs a safe explanation for an index with nonzero conflict stages.
    pub fn conflict(path: PathBuf, kind: DiffKind) -> Self {
        let mut snapshot = Self::new(path, kind, Document::absent(), Document::absent());
        snapshot.presentation = Presentation::Conflict;
        snapshot
    }

    /// Computes one aligned diff through gix; caller never runs a second widget diff.
    pub fn new(path: PathBuf, kind: DiffKind, left: Document, right: Document) -> Self {
        let mut snapshot = Self {
            path,
            kind,
            left,
            right,
            presentation: Presentation::Text,
            rows: Vec::new(),
            hunks: Vec::new(),
            backend: "gix",
            guard: None,
        };
        if snapshot.left.unsupported || snapshot.right.unsupported {
            snapshot.presentation = Presentation::Unsupported;
            return snapshot;
        }
        if snapshot.left.limited || snapshot.right.limited {
            snapshot.presentation = Presentation::TooLarge;
            return snapshot;
        }
        if snapshot.left.bytes.contains(&0) || snapshot.right.bytes.contains(&0) {
            snapshot.presentation = Presentation::Binary;
            return snapshot;
        }
        let (Ok(left), Ok(right)) = (
            std::str::from_utf8(&snapshot.left.bytes),
            std::str::from_utf8(&snapshot.right.bytes),
        ) else {
            snapshot.presentation = Presentation::InvalidEncoding;
            return snapshot;
        };
        let left_lines: Vec<_> = left.split_inclusive('\n').collect();
        let right_lines: Vec<_> = right.split_inclusive('\n').collect();
        if left_lines.len() + right_lines.len() > MAX_LINES
            || left_lines
                .iter()
                .chain(&right_lines)
                .any(|line| line.len() > 16_384)
        {
            snapshot.presentation = Presentation::TooLarge;
            return snapshot;
        }
        let input = InternedInput::new(lines(left), lines(right));
        let diff = diff_with_slider_heuristics(Algorithm::Histogram, &input);
        let (mut old, mut new) = (0, 0);
        for hunk in diff.hunks() {
            let old_start = hunk.before.start as usize;
            let new_start = hunk.after.start as usize;
            while old < old_start && new < new_start {
                snapshot.rows.push(DiffRow {
                    left: Some(line(left_lines[old], old)),
                    right: Some(line(right_lines[new], new)),
                    changed: false,
                });
                old += 1;
                new += 1;
            }
            let start = snapshot.rows.len();
            let removals = hunk.before.end as usize - hunk.before.start as usize;
            let additions = hunk.after.end as usize - hunk.after.start as usize;
            for offset in 0..removals.max(additions) {
                let left = (offset < removals)
                    .then(|| line(left_lines[old_start + offset], old_start + offset));
                let right = (offset < additions)
                    .then(|| line(right_lines[new_start + offset], new_start + offset));
                snapshot.rows.push(DiffRow {
                    left,
                    right,
                    changed: true,
                });
            }
            snapshot.hunks.push(start..snapshot.rows.len());
            old = hunk.before.end as usize;
            new = hunk.after.end as usize;
        }
        while old < left_lines.len() && new < right_lines.len() {
            snapshot.rows.push(DiffRow {
                left: Some(line(left_lines[old], old)),
                right: Some(line(right_lines[new], new)),
                changed: false,
            });
            old += 1;
            new += 1;
        }
        snapshot
    }
}
