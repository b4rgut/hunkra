//! Owned history requests/replies; the host supplies a bounded reader to the native view.
use crate::{history_state::HistoryService, worker::LatestWorker};
use hunkra_core::{CommitBase, CommitDetails, CoreError, DiffSnapshot, HistoryPage};
use std::{path::PathBuf, sync::Arc};

pub enum HistoryQuery {
    Page {
        root: PathBuf,
        session: u64,
        offset: usize,
    },
    Commit {
        root: PathBuf,
        id: String,
        base: CommitBase,
    },
    Diff(Arc<CommitDetails>, PathBuf),
}

pub enum HistoryRead {
    Page(Arc<HistoryPage>),
    Commit(Arc<CommitDetails>),
    Diff(Arc<DiffSnapshot>),
}

pub type HistoryWorker = LatestWorker<HistoryQuery, Result<HistoryRead, CoreError>>;

/// Creates the standalone gix service on its own thread; embedded hosts can supply their existing source instead.
pub fn standalone_reader() -> HistoryWorker {
    LatestWorker::with_state(
        "hunkra-history",
        HistoryService::default,
        |service, query| match query {
            HistoryQuery::Page {
                root,
                session,
                offset,
            } => service.page(&root, session, offset).map(HistoryRead::Page),
            HistoryQuery::Commit { root, id, base } => hunkra_core::read_commit(&root, &id, base)
                .map(|details| HistoryRead::Commit(Arc::new(details))),
            HistoryQuery::Diff(details, path) => hunkra_core::read_commit_diff(&details, &path)
                .map(|diff| HistoryRead::Diff(Arc::new(diff))),
        },
    )
}
