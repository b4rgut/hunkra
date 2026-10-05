//! Read-only gix adapter. Merely viewing a repository never writes its index.
use crate::diff::{DiffKind, DiffSnapshot, Document, MAX_BYTES};
use gix::bstr::ByteSlice;
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("git operation failed: {0}")]
    Git(String),
    #[error("filesystem operation failed")]
    Io(#[from] std::io::Error),
    #[error("a working tree is required")]
    Bare,
    #[error("invalid repository-relative path")]
    InvalidPath,
    #[error("the repository exceeds the supported file limit")]
    TooManyFiles,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    TypeChanged,
    Renamed,
    Conflict,
    IntentToAdd,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileStatus {
    pub path: PathBuf,
    pub staged: Option<ChangeKind>,
    pub unstaged: Option<ChangeKind>,
    pub conflicted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Merge,
    Rebase,
    Other,
}

#[derive(Clone, Debug)]
pub struct RepositorySnapshot {
    pub root: PathBuf,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub unborn: bool,
    pub operation: Option<Operation>,
    pub files: Vec<FileStatus>,
    pub backend: &'static str,
}

/// Keeps library diagnostics out of the translatable domain error categories.
fn git_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::Git(error.to_string())
}

/// Discovers a repository using gix and returns its canonical worktree root.
fn open(path: &Path) -> Result<gix::Repository, CoreError> {
    let repo = gix::discover(path).map_err(git_error)?;
    if repo.workdir().is_none() {
        return Err(CoreError::Bare);
    }
    Ok(repo)
}

/// Scans both comparisons without persisting gix's optional index stat updates.
pub fn scan(path: &Path) -> Result<RepositorySnapshot, CoreError> {
    let repo = open(path)?;
    let root = repo.workdir().ok_or(CoreError::Bare)?.canonicalize()?;
    let head = repo.head().map_err(git_error)?;
    let branch = head.referent_name().map(|name| name.shorten().to_string());
    let unborn = head.is_unborn();
    let head_id = head.id().map(|id| id.to_string());
    let index = repo.index_or_empty().map_err(git_error)?;
    let mut files = BTreeMap::<PathBuf, FileStatus>::new();
    for entry in index.entries() {
        if entry.stage() != gix::index::entry::Stage::Unconflicted {
            let path = gix::path::from_bstr(entry.path(&index)).into_owned();
            files.entry(path.clone()).or_insert(FileStatus {
                path,
                staged: None,
                unstaged: Some(ChangeKind::Conflict),
                conflicted: true,
            });
        }
    }
    let items = repo
        .status(gix::progress::Discard)
        .map_err(git_error)?
        .untracked_files(gix::status::UntrackedFiles::Files)
        .tree_index_track_renames(gix::status::tree_index::TrackRenames::Disabled)
        .index_worktree_options_mut(|opts| opts.thread_limit = Some(1))
        .into_iter(Vec::<gix::bstr::BString>::new())
        .map_err(git_error)?;
    for item in items {
        let item = item.map_err(git_error)?;
        let (staged, change) = match &item {
            gix::status::Item::TreeIndex(change) => {
                use gix::diff::index::ChangeRef::*;
                (
                    true,
                    Some(match change {
                        Addition { .. } => ChangeKind::Added,
                        Deletion { .. } => ChangeKind::Deleted,
                        Modification { .. } => ChangeKind::Modified,
                        Rewrite { .. } => ChangeKind::Renamed,
                    }),
                )
            }
            gix::status::Item::IndexWorktree(change) => {
                use gix::status::index_worktree::iter::Summary::*;
                (
                    false,
                    change.summary().map(|summary| match summary {
                        Added => ChangeKind::Added,
                        Removed => ChangeKind::Deleted,
                        Modified => ChangeKind::Modified,
                        TypeChange => ChangeKind::TypeChanged,
                        Conflict => ChangeKind::Conflict,
                        IntentToAdd => ChangeKind::IntentToAdd,
                        Renamed | Copied => ChangeKind::Renamed,
                    }),
                )
            }
        };
        let Some(change) = change else {
            continue;
        };
        let path = gix::path::from_bstr(item.location()).into_owned();
        let file = files.entry(path.clone()).or_insert(FileStatus {
            path,
            staged: None,
            unstaged: None,
            conflicted: false,
        });
        if !file.conflicted {
            if staged {
                file.staged = Some(change);
            } else {
                file.unstaged = Some(change);
            }
        }
        if files.len() > 100_000 {
            return Err(CoreError::TooManyFiles);
        }
    }
    let operation = repo.state().map(|state| match state {
        gix::state::InProgress::Merge => Operation::Merge,
        gix::state::InProgress::Rebase
        | gix::state::InProgress::RebaseInteractive
        | gix::state::InProgress::ApplyMailboxRebase => Operation::Rebase,
        _ => Operation::Other,
    });
    Ok(RepositorySnapshot {
        root,
        branch,
        head: head_id,
        unborn,
        operation,
        files: files.into_values().collect(),
        backend: "gix",
    })
}

/// Refuses traversal and intermediate symlinks outside the worktree.
fn check_path(root: &Path, path: &Path) -> Result<(), CoreError> {
    if path.as_os_str().is_empty() || !path.components().all(|p| matches!(p, Component::Normal(_)))
    {
        return Err(CoreError::InvalidPath);
    }
    // Walk existing parents: deleted files may have absent directories.
    let mut parent = root.join(path).parent().map(Path::to_path_buf);
    while let Some(candidate) = parent {
        if candidate.exists() {
            if !candidate.canonicalize()?.starts_with(root) {
                return Err(CoreError::InvalidPath);
            }
            break;
        }
        parent = candidate.parent().map(Path::to_path_buf);
    }
    Ok(())
}

/// Reads a bounded object, retaining presence, mode and size even for large blobs.
fn blob(repo: &gix::Repository, id: gix::ObjectId, mode: u32) -> Result<Document, CoreError> {
    let header = repo.find_header(id).map_err(git_error)?;
    let size = header.size();
    if size > MAX_BYTES as u64 {
        return Ok(Document::oversized(size, Some(mode)));
    }
    let object = repo.find_object(id).map_err(git_error)?;
    if object.kind != gix::objs::Kind::Blob {
        return Ok(Document::unsupported(Some(mode)));
    }
    Ok(Document::present(object.data.to_vec(), Some(mode)))
}

/// Resolves a file at HEAD; unborn HEAD and deleted paths have an empty side.
fn at_head(repo: &gix::Repository, path: &Path) -> Result<Document, CoreError> {
    if repo.head().map_err(git_error)?.is_unborn() {
        return Ok(Document::absent());
    }
    let tree = repo.head_tree().map_err(git_error)?;
    match tree.lookup_entry_by_path(path).map_err(git_error)? {
        Some(entry) => blob(repo, entry.object_id(), entry.mode().value() as u32),
        None => Ok(Document::absent()),
    }
}

/// Reads the unconflicted index entry without collapsing missing entries into errors.
fn at_index(
    repo: &gix::Repository,
    index: &gix::index::State,
    path: &Path,
) -> Result<Document, CoreError> {
    let bytes = gix::path::into_bstr(path);
    match index.entry_by_path_and_stage(bytes.as_ref(), gix::index::entry::Stage::Unconflicted) {
        Some(entry)
            if entry
                .flags
                .contains(gix::index::entry::Flags::INTENT_TO_ADD) =>
        {
            Ok(Document::absent())
        }
        Some(entry) => blob(repo, entry.id, entry.mode.bits()),
        None => Ok(Document::absent()),
    }
}

/// Reads a worktree file in Git representation using gix attributes/clean conversion.
fn at_worktree(
    repo: &gix::Repository,
    index: &gix::index::State,
    root: &Path,
    path: &Path,
) -> Result<Document, CoreError> {
    let full_path = root.join(path);
    let metadata = match fs::symlink_metadata(&full_path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Document::absent()),
        Err(error) => return Err(error.into()),
    };
    if metadata.is_symlink() {
        let target = fs::read_link(full_path)?;
        return Ok(Document::present(
            target.as_os_str().as_encoded_bytes().to_vec(),
            Some(0o120000),
        ));
    }
    if !metadata.is_file() {
        return Ok(Document::unsupported(Some(0o160000)));
    }
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 != 0 {
            0o100755
        } else {
            0o100644
        }
    };
    #[cfg(not(unix))]
    let mode = 0o100644;
    if metadata.len() > MAX_BYTES as u64 {
        return Ok(Document::oversized(metadata.len(), Some(mode)));
    }
    let raw = fs::File::open(full_path)?.take(MAX_BYTES as u64 + 1);
    let (mut pipeline, _) = repo.filter_pipeline(None).map_err(git_error)?;
    let outcome = pipeline
        .convert_to_git(raw, path, index)
        .map_err(git_error)?;
    let mut bytes = Vec::new();
    outcome.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BYTES {
        return Ok(Document::oversized(bytes.len() as u64, Some(mode)));
    }
    Ok(Document::present(bytes, Some(mode)))
}

/// Reads exactly HEAD→index or index→worktree for one repository-relative path.
pub fn read_diff(root: &Path, path: &Path, kind: DiffKind) -> Result<DiffSnapshot, CoreError> {
    let repo = open(root)?;
    let root = repo.workdir().ok_or(CoreError::Bare)?.canonicalize()?;
    check_path(&root, path)?;
    let index = repo.index_or_empty().map_err(git_error)?;
    let bytes = gix::path::into_bstr(path);
    let conflict = index.entries().iter().any(|entry| {
        entry.path(&index) == bytes.as_bstr()
            && entry.stage() != gix::index::entry::Stage::Unconflicted
    });
    if conflict {
        return Ok(DiffSnapshot::conflict(path.to_path_buf(), kind));
    }
    let left = match kind {
        DiffKind::Staged => at_head(&repo, path)?,
        DiffKind::Unstaged => at_index(&repo, &index, path)?,
    };
    let right = match kind {
        DiffKind::Staged => at_index(&repo, &index, path)?,
        DiffKind::Unstaged => at_worktree(&repo, &index, &root, path)?,
    };
    Ok(DiffSnapshot::new(path.to_path_buf(), kind, left, right))
}
