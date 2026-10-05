//! Git and diff services. Public models contain no GPUI or gix types.
pub mod commit;
pub mod diff;
mod index_format;
pub mod repository;
pub mod staging;

pub use diff::{DiffKind, DiffRow, DiffSnapshot, Document, Line, Presentation};
pub use repository::{
    ChangeKind, CoreError, FileStatus, IndexLimitation, Operation, RepositorySnapshot, read_diff,
    scan,
};

pub use commit::{CommitSnapshot, CommitWrite, create_commit, prepare_commit};
pub use staging::{IndexWrite, LineSelection, Selection, update_index};
