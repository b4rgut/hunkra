//! Git and diff services. Public models contain no GPUI or gix types.
pub mod branches;
pub mod commit;
pub mod conflict_chunks;
pub mod conflicts;
pub mod diff;
pub mod diff_reader;
pub mod history;
mod index_format;
pub mod network;
pub mod operations;
pub mod repository;
pub mod staging;
pub mod transitions;

pub use diff::{DiffKind, DiffRow, DiffSnapshot, Document, Line, Presentation};
pub use diff_reader::{CachedDiff, DiffCacheStats, DiffReader};
pub use repository::{
    ChangeKind, CoreError, FileStatus, IndexLimitation, Operation, RepositorySnapshot, read_diff,
    scan,
};

pub use branches::{Branch, BranchSnapshot, BranchWrite, create_branch, read_branches};
pub use commit::{CommitSnapshot, CommitWrite, create_commit, prepare_commit};
pub use conflicts::{
    ConflictSide, ConflictSnapshot, ConflictWrite, mark_resolved, read_conflict, save_result,
};
pub use history::{
    CommitBase, CommitDetails, CommitFile, CommitSummary, HistoryPage, HistoryReader, read_commit,
    read_commit_diff,
};
pub use operations::{OperationSnapshot, OperationState, prepare_operation};
pub use staging::{IndexWrite, LineSelection, Selection, update_index};
pub use transitions::{
    OperationAction, OperationCapability, OperationOutcome, OperationRoute, OperationWrite,
    operation_route, run_operation,
};
