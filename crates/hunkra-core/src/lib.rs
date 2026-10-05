//! Git and diff services. Public models contain no GPUI or gix types.
pub mod diff;
pub mod repository;
pub mod staging;

pub use diff::{DiffKind, DiffRow, DiffSnapshot, Document, Line, Presentation};
pub use repository::{
    ChangeKind, CoreError, FileStatus, Operation, RepositorySnapshot, read_diff, scan,
};

pub use staging::{IndexWrite, LineSelection, Selection, update_index};
