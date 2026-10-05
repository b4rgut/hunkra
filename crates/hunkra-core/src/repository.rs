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
    #[error("the diff sources have changed; refresh before writing")]
    Stale,
    #[error("this repository or file variant is not supported for index writes")]
    UnsupportedWrite,
    #[error("index capability is not supported: {0:?}")]
    IndexVariant(IndexLimitation),
    #[error("select both sides of this EOF replacement to preserve line boundaries")]
    SelectionBoundary,
    #[error("no changed lines selected")]
    EmptySelection,
    #[error("could not acquire the index lock")]
    IndexLock,
    #[error("commit message is empty, invalid or exceeds the supported limit")]
    CommitMessage,
    #[error("there are no staged changes to commit")]
    NothingStaged,
    #[error("commit identity is missing or invalid")]
    Identity,
    #[error("unresolved index stages must be resolved before committing")]
    Unresolved,
    #[error("complete the current Git operation before starting another action")]
    ActiveOperation,
    #[error("Git hook failed: {0}")]
    Hook(&'static str),
    #[error("commit signing failed")]
    Signing,
    #[error("reference update failed; inspect current HEAD before retrying")]
    ReferenceUpdate,
    #[error("this commit configuration is not supported: {0}")]
    CommitUnsupported(&'static str),
    #[error("the local branch name is invalid")]
    BranchName,
    #[error("the local branch already exists")]
    BranchExists,
    #[error("a committed HEAD is required for this action")]
    Unborn,
    #[error("the reference set is unsupported or exceeds its limit")]
    ReferenceLimit,
    #[error("the operation metadata is invalid, unsafe or exceeds its limit")]
    OperationMetadata,
    #[error("the selected local branch no longer exists")]
    BranchMissing,
    #[error("there is no active Git operation")]
    NoOperation,
    #[error("this action does not apply to the current operation")]
    OperationAction,
    #[error("the Git coordinator failed; refresh the actual state before retrying")]
    OperationExecution {
        capability: crate::OperationCapability,
        exit_code: Option<i32>,
    },
    #[error("the selected file has no recoverable conflict stages")]
    NoConflict,
    #[error("this conflict document cannot be edited safely")]
    ConflictUnsupported,
    #[error("the selected conflict chunk is malformed or has changed")]
    ConflictChunk,
    #[error("joining these versions requires an explicit newline")]
    ConflictBoundary,
    #[error("could not acquire the working file lock")]
    WorktreeLock,
    #[error("history objects or metadata exceed the supported limits")]
    HistoryLimit,
    #[error("the history request is invalid or its reader must be restarted")]
    HistoryRequest,
    #[error("select an actual parent of this commit, or the empty tree for a root commit")]
    HistoryBase,
    #[error("the destination already exists or its repository metadata is unsafe")]
    RepositoryDestination,
    #[error("remote configuration is invalid, unsupported or exceeds its limits")]
    NetworkConfig,
    #[error("select a configured remote and a valid branch")]
    NetworkTarget,
    #[error("the network operation failed; inspect actual state before retrying")]
    NetworkExecution,
    #[error("some remote references were rejected; inspect the fetched state")]
    FetchRejected,
    #[error("the clone checkout is incomplete; inspect its preserved repository")]
    CloneCheckout,
    #[error("the operation was canceled; inspect any partial changes")]
    NetworkCanceled,
    #[error("this action requires a current local branch and one configured upstream")]
    Upstream,
    #[error("could not acquire the local repository configuration lock")]
    ConfigLock,
    #[error("the selected network coordinator failed; inspect actual state before retrying")]
    NetworkCoordinator {
        capability: crate::network::NetworkCapability,
        exit_code: Option<i32>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexLimitation {
    CaseFolding,
    UnicodeComposition,
    Sparse,
    Split,
    SkipWorktree,
    PathCollision,
    SharedPermissions,
    IndexHook,
    Symlink,
    UnknownExtension,
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
    pub commit: crate::CommitSnapshot,
}

/// Keeps library diagnostics out of the translatable domain error categories.
pub(crate) fn git_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::Git(error.to_string())
}

/// Discovers a repository using gix and returns its canonical worktree root.
pub(crate) fn open(path: &Path) -> Result<gix::Repository, CoreError> {
    let repo = gix::discover(path).map_err(git_error)?;
    if repo.workdir().is_none() {
        return Err(CoreError::Bare);
    }
    Ok(repo)
}

/// Resolves existing symbolic HEAD chains; only a genuinely unborn HEAD has no object id.
pub(crate) fn resolved_head(repo: &gix::Repository) -> Result<Option<String>, CoreError> {
    if repo.head().map_err(git_error)?.is_unborn() {
        return Ok(None);
    }
    Ok(Some(repo.head_id().map_err(git_error)?.to_string()))
}

/// Scans both comparisons without persisting gix's optional index stat updates.
pub fn scan(path: &Path) -> Result<RepositorySnapshot, CoreError> {
    let repo = open(path)?;
    let commit = crate::prepare_commit(repo.workdir().ok_or(CoreError::Bare)?)?;
    let root = repo.workdir().ok_or(CoreError::Bare)?.canonicalize()?;
    let head = repo.head().map_err(git_error)?;
    let branch = head.referent_name().map(|name| name.shorten().to_string());
    let unborn = head.is_unborn();
    let head_id = resolved_head(&repo)?;
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
    if crate::prepare_commit(&root)? != commit {
        return Err(CoreError::Stale);
    }
    Ok(RepositorySnapshot {
        root,
        branch,
        head: head_id,
        unborn,
        operation,
        files: files.into_values().collect(),
        backend: "gix",
        commit,
    })
}

/// Validates repository-relative syntax independently of the current working filesystem.
pub(crate) fn relative_path(path: &Path) -> Result<(), CoreError> {
    if path.as_os_str().is_empty()
        || path.as_os_str().as_encoded_bytes().contains(&0)
        || !path.components().all(|component| match component {
            Component::Normal(name) => !name.as_encoded_bytes().eq_ignore_ascii_case(b".git"),
            _ => false,
        })
    {
        return Err(CoreError::InvalidPath);
    }
    Ok(())
}

/// Refuses traversal and intermediate symlinks outside the worktree.
pub(crate) fn check_path(root: &Path, path: &Path) -> Result<(), CoreError> {
    relative_path(path)?;
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
pub(crate) fn blob(
    repo: &gix::Repository,
    id: gix::ObjectId,
    mode: u32,
) -> Result<Document, CoreError> {
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
pub(crate) fn at_index(
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
pub(crate) fn at_worktree(
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
    let options = repo.filesystem_options().map_err(git_error)?;
    let path_bytes = gix::path::into_bstr(path);
    // Git add retains an existing mode with fileMode=false, preferring stage 2, then 1, then 3.
    use gix::index::entry::Stage;
    let indexed_mode = [Stage::Unconflicted, Stage::Ours, Stage::Base, Stage::Theirs]
        .into_iter()
        .find_map(|stage| index.entry_by_path_and_stage(path_bytes.as_bstr(), stage))
        .map_or(gix::index::entry::Mode::FILE, |entry| entry.mode);
    #[cfg(unix)]
    let executable = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    };
    #[cfg(not(unix))]
    let executable = false;
    let mode = indexed_mode
        .change_to_match_fs_with_values(
            true,
            false,
            false,
            executable,
            options.symlink,
            options.executable_bit,
        )
        .map_or(indexed_mode, |change| change.apply(indexed_mode))
        .bits();
    if metadata.len() > MAX_BYTES as u64 {
        return Ok(Document::oversized(metadata.len(), Some(mode)));
    }
    let raw = fs::File::open(full_path)?.take(MAX_BYTES as u64 + 1);
    // core.symlinks=false checks a tracked link out as a regular file of target bytes.
    if mode == 0o120000 {
        let mut bytes = Vec::new();
        raw.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
        return Ok(Document::present(bytes, Some(mode)));
    }
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
    let guard = capture_guard(&repo, &root, path)?;
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
    if capture_guard(&repo, &root, path)? != guard {
        return Err(CoreError::Stale);
    }
    let mut snapshot = DiffSnapshot::new(path.to_path_buf(), kind, left, right);
    snapshot.guard = Some(guard);
    Ok(snapshot)
}

/// Opaque source precondition; domain/UI snapshots expose no gix IDs or index handles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RepositoryGuard {
    pub root: PathBuf,
    pub index: Option<Vec<u8>>,
    pub head: Option<String>,
    pub raw_worktree: Option<Vec<u8>>,
    pub mode: Option<u32>,
}

/// Hashes exact index contents, including extensions, using a fixed independent digest.
pub(crate) fn index_digest(path: &Path) -> Result<Option<Vec<u8>>, CoreError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let hash = gix::hash::bytes_of_file(
        path,
        metadata.len(),
        gix::hash::Kind::Sha256,
        &mut gix::progress::Discard,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .map_err(git_error)?;
    Ok(Some(hash.as_slice().to_vec()))
}

/// Captures raw file bytes separately from normalized Git bytes for stale-diff checks.
pub(crate) fn capture_guard(
    repo: &gix::Repository,
    root: &Path,
    path: &Path,
) -> Result<RepositoryGuard, CoreError> {
    check_path(root, path)?;
    let full_path = root.join(path);
    let (raw_worktree, mode) = match fs::symlink_metadata(&full_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (None, None),
        Err(error) => return Err(error.into()),
        Ok(metadata) if metadata.is_symlink() => {
            let target = fs::read_link(&full_path)?;
            (
                Some(target.as_os_str().as_encoded_bytes().to_vec()),
                Some(0o120000),
            )
        }
        Ok(metadata) if metadata.is_file() => {
            let mut bytes = Vec::new();
            fs::File::open(full_path)?
                .take(MAX_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
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
            (Some(bytes), Some(mode))
        }
        Ok(_) => (None, Some(0o160000)),
    };
    Ok(RepositoryGuard {
        root: root.to_path_buf(),
        index: index_digest(&repo.index_path())?,
        head: resolved_head(repo)?,
        raw_worktree,
        mode,
    })
}
