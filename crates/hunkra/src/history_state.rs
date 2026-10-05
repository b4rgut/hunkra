//! Bounded history page retention and idempotent worker requests, independent of GPUI.
use hunkra_core::{CoreError, HistoryPage, HistoryReader};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::Arc,
};

pub const PAGE_SIZE: usize = 100;
const CACHED_PAGES: usize = 8;

#[derive(Default)]
pub struct HistoryService {
    reader: Option<HistoryReader>,
    root: Option<PathBuf>,
    session: u64,
    last: Option<Arc<HistoryPage>>,
}
impl HistoryService {
    /// Reuses the last page after a dropped reply, refusing skipped offsets before advancing traversal.
    pub fn page(
        &mut self,
        root: &Path,
        session: u64,
        offset: usize,
    ) -> Result<Arc<HistoryPage>, CoreError> {
        if self.session != session || self.root.as_deref() != Some(root) || self.reader.is_none() {
            if offset != 0 {
                return Err(CoreError::HistoryRequest);
            }
            self.reader = Some(HistoryReader::open(root)?);
            self.root = Some(root.to_path_buf());
            self.session = session;
            self.last = None;
        }
        if let Some(page) = &self.last {
            if page.offset == offset {
                return Ok(page.clone());
            }
            if page.offset + page.commits.len() != offset || !page.has_more {
                return Err(CoreError::HistoryRequest);
            }
        } else if offset != 0 {
            return Err(CoreError::HistoryRequest);
        }
        let page = Arc::new(
            self.reader
                .as_mut()
                .expect("initialized reader")
                .next_page(PAGE_SIZE)?,
        );
        self.last = Some(page.clone());
        Ok(page)
    }
}

#[derive(Default)]
pub struct HistoryCache {
    pages: VecDeque<Arc<HistoryPage>>,
    active: usize,
}
impl HistoryCache {
    /// Drops only history pages for a new immutable-tip session; host Result/message are separate.
    pub fn clear(&mut self) {
        self.pages.clear();
        self.active = 0;
    }
    /// Accepts only the expected consecutive page of the same root/tip; stale or skipped replies fail.
    pub fn install(&mut self, page: Arc<HistoryPage>) -> bool {
        if page.commits.len() > PAGE_SIZE {
            return false;
        }
        if let Some(last) = self.pages.back() {
            if last.root != page.root
                || last.tip != page.tip
                || !last.has_more
                || last.offset + last.commits.len() != page.offset
            {
                return false;
            }
        } else if page.offset != 0 {
            return false;
        }
        self.pages.push_back(page);
        if self.pages.len() > CACHED_PAGES {
            self.pages.pop_front();
        }
        self.active = self.pages.len() - 1;
        true
    }
    /// Exposes only the currently displayed bounded page, without flattening the entire history.
    pub fn current(&self) -> Option<&Arc<HistoryPage>> {
        self.pages.get(self.active)
    }
    /// Reports the next uncached offset only while the latest displayed page has more ancestors.
    pub fn next_offset(&self) -> Option<usize> {
        let page = self.current()?;
        (self.active + 1 == self.pages.len() && page.has_more)
            .then_some(page.offset + page.commits.len())
    }
    /// Moves one cached page toward newer commits without performing Git work.
    pub fn previous(&mut self) -> bool {
        if self.active == 0 {
            return false;
        }
        self.active -= 1;
        true
    }
    /// Moves one cached page toward older commits without advancing the worker cursor twice.
    pub fn next_cached(&mut self) -> bool {
        if self.active + 1 >= self.pages.len() {
            return false;
        }
        self.active += 1;
        true
    }
    /// Enables page navigation only when an actual cached or unseen page exists.
    pub fn can_previous(&self) -> bool {
        self.active > 0
    }
    /// Enables loading older ancestors independently of commit/file selection.
    pub fn can_next(&self) -> bool {
        self.active + 1 < self.pages.len() || self.next_offset().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hunkra_core::CommitSummary;
    #[test]
    /// A replaced mailbox reply can be requested again without silently skipping the next history page.
    fn dropped_page_reply_is_idempotent_and_skipped_offsets_are_refused() {
        let fixture = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("/usr/bin/git")
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", fixture.path())
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .current_dir(fixture.path())
                .args([
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "-c",
                    "core.hooksPath=/dev/null",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            output.stdout
        };
        git(&["init", "-b", "main"]);
        for number in 0..111 {
            git(&["commit", "--allow-empty", "-m", &number.to_string()]);
        }
        let mut service = HistoryService::default();
        let first = service.page(fixture.path(), 1, 0).unwrap();
        let repeated = service.page(fixture.path(), 1, 0).unwrap();
        assert!(Arc::ptr_eq(&first, &repeated));
        assert!(matches!(
            service.page(fixture.path(), 1, 99),
            Err(CoreError::HistoryRequest)
        ));
        assert!(matches!(
            service.page(fixture.path(), 1, 101),
            Err(CoreError::HistoryRequest)
        ));
        let second = service.page(fixture.path(), 1, 100).unwrap();
        assert_eq!(second.offset, 100);
        assert_eq!(second.commits.len(), 11);
        assert!(!second.has_more);
        assert!(Arc::ptr_eq(
            &second,
            &service.page(fixture.path(), 1, 100).unwrap()
        ));
        assert!(matches!(
            service.page(fixture.path(), 1, 111),
            Err(CoreError::HistoryRequest)
        ));
        assert!(matches!(
            service.page(fixture.path(), 2, 100),
            Err(CoreError::HistoryRequest)
        ));
        let reloaded = service.page(fixture.path(), 2, 0).unwrap();
        assert_eq!(reloaded.commits[0].id, first.commits[0].id);
        assert!(!Arc::ptr_eq(&first, &reloaded));
    }
    /// Constructs metadata-only pages to exercise retention without any repository side effects.
    fn page(offset: usize, tip: &str) -> Arc<HistoryPage> {
        Arc::new(HistoryPage {
            root: PathBuf::from("/fixture"),
            tip: Some(tip.into()),
            offset,
            has_more: true,
            commits: (0..PAGE_SIZE)
                .map(|index| CommitSummary {
                    id: format!("{offset}-{index}"),
                    parents: vec![],
                    subject: None,
                    author: None,
                    commit_seconds: 0,
                })
                .collect(),
        })
    }
    #[test]
    /// Same-tip consecutive pages stay bounded; backward/forward navigation never invents offsets.
    fn consecutive_cache_is_bounded_and_navigation_is_local() {
        let mut cache = HistoryCache::default();
        for index in 0..12 {
            assert!(cache.install(page(index * PAGE_SIZE, "tip")));
        }
        assert_eq!(cache.pages.len(), CACHED_PAGES);
        assert_eq!(cache.next_offset(), Some(12 * PAGE_SIZE));
        for _ in 1..CACHED_PAGES {
            assert!(cache.previous());
            assert_eq!(cache.next_offset(), None);
        }
        assert!(!cache.previous());
        assert_eq!(cache.current().unwrap().offset, 4 * PAGE_SIZE);
        assert!(cache.next_cached());
        assert_eq!(cache.current().unwrap().offset, 5 * PAGE_SIZE);
        assert!(cache.can_previous() && cache.can_next());
    }
    #[test]
    /// Stale tips, repeated/skipped offsets and excessive rows leave the displayed page untouched.
    fn wrong_replies_do_not_replace_reviewed_history() {
        let mut cache = HistoryCache::default();
        assert!(!cache.install(page(PAGE_SIZE, "tip")));
        assert!(cache.install(page(0, "tip")));
        assert!(!cache.install(page(0, "tip")));
        assert!(!cache.install(page(2 * PAGE_SIZE, "tip")));
        assert!(!cache.install(page(PAGE_SIZE, "changed")));
        let mut excessive = page(PAGE_SIZE, "tip");
        let oversized = Arc::get_mut(&mut excessive).unwrap();
        oversized.commits.push(oversized.commits[0].clone());
        assert!(!cache.install(excessive));
        assert_eq!(cache.current().unwrap().offset, 0);
        cache.clear();
        assert!(cache.current().is_none());
        assert!(cache.install(page(0, "new tip")));
    }
}
