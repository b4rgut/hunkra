//! Case-insensitive path terms are prepared and matched off the UI thread; rendering uses original snapshot indices.
use crate::worker::LatestWorker;
use std::{path::Path, sync::Arc};

pub struct PathIndex {
    folded: Vec<String>,
    all: Arc<Vec<usize>>,
}
impl PathIndex {
    /// Prepares searchable display paths once on a status worker; original repository paths remain untouched.
    pub fn new<'a>(paths: impl Iterator<Item = &'a Path>) -> Self {
        let folded = paths
            .map(|path| path.to_string_lossy().to_lowercase())
            .collect::<Vec<_>>();
        let all = Arc::new((0..folded.len()).collect());
        Self { folded, all }
    }
    /// Requires every whitespace-separated term while preserving repository order and original row identity.
    fn matching(&self, query: &str) -> Arc<Vec<usize>> {
        let query = query.to_lowercase();
        let terms = query.split_whitespace().collect::<Vec<_>>();
        if terms.is_empty() {
            return self.all.clone();
        }
        Arc::new(
            self.folded
                .iter()
                .enumerate()
                .filter_map(|(index, path)| {
                    terms
                        .iter()
                        .all(|term| path.contains(term))
                        .then_some(index)
                })
                .collect(),
        )
    }
}

pub struct PathFilter {
    worker: LatestWorker<(Arc<PathIndex>, String), Arc<Vec<usize>>>,
    index: Option<Arc<PathIndex>>,
    generation: u64,
    query_text: String,
    pub rows: Arc<Vec<usize>>,
    pub loading: bool,
}
impl Default for PathFilter {
    /// Creates one bounded matcher; typing coalesces obsolete queries without blocking Git readers or rendering.
    fn default() -> Self {
        Self {
            worker: LatestWorker::new(
                "hunkra-filter",
                |(index, query): (Arc<PathIndex>, String)| index.matching(&query),
            ),
            index: None,
            generation: 0,
            query_text: String::new(),
            rows: Arc::new(Vec::new()),
            loading: false,
        }
    }
}
impl PathFilter {
    /// Invalidates all previous indices when the repository snapshot changes, preventing old rows from indexing a new snapshot.
    pub fn replace(&mut self, index: Option<Arc<PathIndex>>, query: &str) {
        if self.query_text == query
            && self
                .index
                .as_ref()
                .zip(index.as_ref())
                .is_some_and(|(previous, current)| previous.folded == current.folded)
        {
            self.index = index;
            return;
        }
        self.index = index;
        self.query(query);
    }
    /// Keeps the selected document/draft intact while matching only the sidebar; no Git work is requested.
    pub fn query(&mut self, query: &str) {
        self.generation += 1;
        self.query_text = query.to_owned();
        self.rows = Arc::new(Vec::new());
        self.loading = false;
        if let Some(index) = &self.index {
            if query.trim().is_empty() {
                self.rows = index.all.clone();
            } else {
                self.loading = true;
                self.worker
                    .submit(self.generation, (index.clone(), query.to_owned()));
            }
        }
    }
    /// Applies only the latest query/snapshot reply, discarding obsolete results from rapid typing or project switches.
    pub fn poll(&mut self) -> bool {
        if let Some((generation, rows)) = self.worker.take()
            && generation == self.generation
        {
            self.rows = rows;
            self.loading = false;
            return true;
        }
        false
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    /// Unicode lowercase matching, multiple terms and empty/no-match queries keep stable source indices without changing paths.
    fn terms_preserve_repository_order() {
        let paths = [
            Path::new("src/Ядро.rs"),
            Path::new("docs/Ядро.md"),
            Path::new("src/main.rs"),
        ];
        let index = PathIndex::new(paths.into_iter());
        assert_eq!(*index.matching("SRC ядРО"), [0]);
        assert_eq!(*index.matching(".rs"), [0, 2]);
        assert_eq!(*index.matching("  \t"), [0, 1, 2]);
        assert!(index.matching("missing").is_empty());
    }
}
