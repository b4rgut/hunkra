//! Bounded worker-owned alignment reuse; every request still rereads and verifies exact Git/worktree sources.
use crate::{CoreError, DiffKind, DiffRow, DiffSnapshot, repository::read_diff_sources};
use std::{
    collections::VecDeque,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

pub const CACHE_ENTRIES: usize = 4;
pub const CACHE_BYTES: usize = 24 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default)]
pub struct DiffCacheStats {
    pub entries: usize,
    pub allocated_bytes: usize,
    pub hits: u64,
    pub misses: u64,
}
pub struct CachedDiff {
    pub snapshot: Arc<DiffSnapshot>,
    pub reused: bool,
    pub source_time: Duration,
    pub prepare_time: Duration,
}
#[derive(Default)]
pub struct DiffReader {
    cache: VecDeque<(Arc<DiffSnapshot>, usize)>,
    stats: DiffCacheStats,
}

/// Accounts retained owned buffer capacities and collection storage; allocator overhead/shared external Arcs are outside this budget.
fn allocated_bytes(diff: &DiffSnapshot) -> usize {
    let documents = diff.left.bytes.capacity() + diff.right.bytes.capacity();
    let rows = diff.rows.capacity() * size_of::<DiffRow>()
        + diff
            .rows
            .iter()
            .map(|row| {
                row.left
                    .iter()
                    .chain(&row.right)
                    .map(|line| line.text.capacity())
                    .sum::<usize>()
            })
            .sum::<usize>();
    let guard = diff.guard.as_ref().map_or(0, |guard| {
        guard.root.capacity()
            + guard.head.as_ref().map_or(0, String::capacity)
            + guard.index.as_ref().map_or(0, Vec::capacity)
            + guard.raw_worktree.as_ref().map_or(0, Vec::capacity)
    });
    documents
        + rows
        + guard
        + diff.hunks.capacity() * size_of::<std::ops::Range<usize>>()
        + diff.path.capacity()
        + size_of::<DiffSnapshot>()
        + 2 * size_of::<usize>()
}
impl DiffReader {
    /// Reports bounded resident cache storage separately from an active/outstanding UI snapshot.
    pub fn stats(&self) -> DiffCacheStats {
        self.stats
    }
    /// Revalidates real bytes, modes, index/HEAD and filter output before reusing alignment; errors never return a stale cache entry.
    pub fn read(
        &mut self,
        root: &Path,
        path: &Path,
        kind: DiffKind,
    ) -> Result<CachedDiff, CoreError> {
        let start = Instant::now();
        let sources = read_diff_sources(root, path, kind)?;
        let source_time = start.elapsed();
        let start = Instant::now();
        if let Some(index) = self
            .cache
            .iter()
            .position(|(previous, _)| sources.matches(previous))
        {
            let entry = self.cache.remove(index).expect("existing cached diff");
            let snapshot = entry.0.clone();
            self.cache.push_front(entry);
            self.stats.hits = self.stats.hits.saturating_add(1);
            return Ok(CachedDiff {
                snapshot,
                reused: true,
                source_time,
                prepare_time: start.elapsed(),
            });
        }
        self.stats.misses = self.stats.misses.saturating_add(1);
        let snapshot = Arc::new(sources.prepare());
        let bytes = allocated_bytes(&snapshot);
        // Keep only one version of a path/comparison; other paths support fast switching.
        self.cache.retain(|(previous, _)| {
            previous.path != snapshot.path
                || previous.kind != kind
                || previous.guard.as_ref().map(|guard| &guard.root)
                    != snapshot.guard.as_ref().map(|guard| &guard.root)
        });
        self.stats.allocated_bytes = self.cache.iter().map(|(_, bytes)| bytes).sum();
        if bytes <= CACHE_BYTES {
            while self.cache.len() >= CACHE_ENTRIES
                || self.stats.allocated_bytes + bytes > CACHE_BYTES
            {
                let (_, removed) = self
                    .cache
                    .pop_back()
                    .expect("cache exceeds its bounded budget");
                self.stats.allocated_bytes -= removed;
            }
            self.cache.push_front((snapshot.clone(), bytes));
            self.stats.allocated_bytes += bytes;
        }
        self.stats.entries = self.cache.len();
        Ok(CachedDiff {
            snapshot,
            reused: false,
            source_time,
            prepare_time: start.elapsed(),
        })
    }
}
