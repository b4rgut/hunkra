//! Incremental immutable-tip history and raw commit-tree comparisons through gix.
use crate::repository::{blob, git_error, open, relative_path};
use crate::{ChangeKind, CoreError, DiffKind, DiffSnapshot, Document};
use gix::{
    bstr::ByteSlice,
    error::ErrorExt,
    objs::{Find, FindHeader},
};
use std::{
    cell::Cell,
    ops::ControlFlow,
    path::{Path, PathBuf},
    rc::Rc,
};

const MAX_COMMIT_BYTES: u64 = 1024 * 1024;
const MAX_FILES: usize = 50_000;
pub const MAX_PAGE_SIZE: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitSummary {
    pub id: String,
    pub parents: Vec<String>,
    pub subject: Option<String>,
    pub author: Option<String>,
    pub commit_seconds: i64,
}
#[derive(Debug)]
pub struct HistoryPage {
    pub root: PathBuf,
    pub tip: Option<String>,
    pub offset: usize,
    pub commits: Vec<CommitSummary>,
    pub has_more: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitBase {
    EmptyTree,
    Parent(usize),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitFile {
    pub path: PathBuf,
    pub change: ChangeKind,
    pub old_mode: Option<u32>,
    pub mode: Option<u32>,
}
#[derive(Clone, Debug)]
pub struct CommitDetails {
    pub root: PathBuf,
    pub summary: CommitSummary,
    pub message: Option<String>,
    pub encoding: Option<String>,
    pub base: CommitBase,
    pub base_id: Option<String>,
    pub files: Vec<CommitFile>,
}

struct BoundedObjects {
    repository: gix::Repository,
    limited: Rc<Cell<bool>>,
}
impl Find for BoundedObjects {
    /// Checks object headers before traversal can allocate an oversized commit body.
    fn try_find<'a>(
        &self,
        id: &gix::hash::oid,
        buffer: &'a mut Vec<u8>,
    ) -> gix::ExnResult<Option<gix::objs::Data<'a>>> {
        if self
            .repository
            .try_header(id)?
            .is_some_and(|header| header.size > MAX_COMMIT_BYTES)
        {
            self.limited.set(true);
            return Err(CoreError::HistoryLimit.raise_erased());
        }
        self.repository.try_find(id, buffer)
    }
}

/// Thread-confined reader; only owned pages cross the application's worker boundary.
pub struct HistoryReader {
    root: PathBuf,
    tip: Option<String>,
    walk: gix::traverse::commit::Simple<BoundedObjects, fn(&gix::hash::oid) -> bool>,
    limited: Rc<Cell<bool>>,
    pending: Option<CommitSummary>,
    offset: usize,
    failed: bool,
}
impl HistoryReader {
    /// Pins the current actual HEAD once; later ref changes cannot move an in-flight history session.
    pub fn open(root: &Path) -> Result<Self, CoreError> {
        let mut repository = open(root)?;
        let root = repository
            .workdir()
            .ok_or(CoreError::Bare)?
            .canonicalize()?;
        repository.object_cache_size(4 * 1024 * 1024);
        let tip = repository
            .head()
            .map_err(git_error)?
            .try_peel_to_id()
            .map_err(git_error)?
            .map(|id| id.detach());
        let limited = Rc::new(Cell::new(false));
        let walk = gix::traverse::commit::Simple::new(
            tip,
            BoundedObjects {
                repository,
                limited: limited.clone(),
            },
        )
        .sorting(gix::traverse::commit::simple::Sorting::ByCommitTime(
            gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
        ))
        .map_err(|error| {
            if limited.get() {
                CoreError::HistoryLimit
            } else {
                git_error(error)
            }
        })?;
        Ok(Self {
            root,
            tip: tip.map(|id| id.to_string()),
            walk,
            limited,
            pending: None,
            offset: 0,
            failed: false,
        })
    }
    /// Decodes one yielded object immediately, before the owned iterator reuses its commit buffer.
    fn next_summary(&mut self) -> Result<Option<CommitSummary>, CoreError> {
        let Some(info) = self.walk.next() else {
            return Ok(None);
        };
        let info = info.map_err(|error| {
            if self.limited.get() {
                CoreError::HistoryLimit
            } else {
                git_error(error)
            }
        })?;
        let decoded = gix::objs::CommitRef::from_bytes(self.walk.commit_data(), info.id.kind())
            .map_err(git_error)?;
        Ok(Some(summary(info.id, &decoded)?))
    }
    /// Advances at most one bounded page plus lookahead, without rewalking previous pages.
    pub fn next_page(&mut self, size: usize) -> Result<HistoryPage, CoreError> {
        if size == 0 || size > MAX_PAGE_SIZE || self.failed {
            return Err(CoreError::HistoryRequest);
        }
        let result = self.page(size);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    /// Retains one decoded lookahead so page boundaries never lose a commit or its metadata.
    fn page(&mut self, size: usize) -> Result<HistoryPage, CoreError> {
        let mut commits = Vec::with_capacity(size);
        if let Some(pending) = self.pending.take() {
            commits.push(pending);
        }
        while commits.len() < size {
            let Some(commit) = self.next_summary()? else {
                break;
            };
            commits.push(commit);
        }
        self.pending = self.next_summary()?;
        let page = HistoryPage {
            root: self.root.clone(),
            tip: self.tip.clone(),
            offset: self.offset,
            has_more: self.pending.is_some(),
            commits,
        };
        self.offset += page.commits.len();
        Ok(page)
    }
}

/// Limits display strings at scalar boundaries, leaving unsupported non-UTF-8 metadata explicit.
fn display(bytes: &[u8], size: usize) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    if text.contains('\0') {
        return None;
    }
    let mut clipped: String = text.chars().take(size).collect();
    if text.chars().count() > size {
        clipped.push('…');
    }
    Some(clipped)
}
/// Extracts owned summary fields from validated immutable commit bytes.
fn summary(
    id: gix::ObjectId,
    decoded: &gix::objs::CommitRef<'_>,
) -> Result<CommitSummary, CoreError> {
    if decoded.parents.len() > 256 {
        return Err(CoreError::HistoryLimit);
    }
    let author = decoded.author().map_err(git_error)?;
    let committer = decoded.committer().map_err(git_error)?;
    let supported_text = utf8_encoding(decoded);
    Ok(CommitSummary {
        id: id.to_string(),
        parents: decoded.parents.iter().map(|id| id.to_string()).collect(),
        subject: supported_text
            .then(|| {
                display(
                    decoded
                        .message
                        .split(|byte| *byte == b'\n')
                        .next()
                        .unwrap_or_default(),
                    256,
                )
            })
            .flatten(),
        author: supported_text
            .then(|| display(author.name.as_bytes(), 256))
            .flatten(),
        commit_seconds: committer.seconds(),
    })
}
/// Respects the declared commit encoding instead of guessing UTF-8 from coincidentally valid bytes.
fn utf8_encoding(decoded: &gix::objs::CommitRef<'_>) -> bool {
    decoded.encoding.is_none_or(|name| {
        name.as_bytes().eq_ignore_ascii_case(b"UTF-8")
            || name.as_bytes().eq_ignore_ascii_case(b"UTF8")
    })
}
/// Accepts full object ids only, preventing moving revspecs and accidental option/ref interpretation.
fn object_id(repo: &gix::Repository, id: &str) -> Result<gix::ObjectId, CoreError> {
    let id = gix::ObjectId::from_hex(id.as_bytes()).map_err(|_| CoreError::HistoryRequest)?;
    if id.kind() != repo.object_hash() {
        return Err(CoreError::HistoryRequest);
    }
    Ok(id)
}
/// Loads one commit only after its header proves the body is within the history metadata bound.
fn commit<'repo>(repo: &'repo gix::Repository, id: &str) -> Result<gix::Commit<'repo>, CoreError> {
    let id = object_id(repo, id)?;
    let header = repo
        .try_find_header(id)
        .map_err(git_error)?
        .ok_or(CoreError::HistoryRequest)?;
    if header.size() > MAX_COMMIT_BYTES {
        return Err(CoreError::HistoryLimit);
    }
    if header.kind() != gix::objs::Kind::Commit {
        return Err(CoreError::HistoryRequest);
    }
    repo.find_commit(id).map_err(git_error)
}
/// Resolves an explicit existing parent or empty tree; merge parents are never silently combined.
fn base_tree<'repo>(
    repo: &'repo gix::Repository,
    decoded: &gix::objs::CommitRef<'_>,
    base: CommitBase,
) -> Result<(gix::Tree<'repo>, Option<String>), CoreError> {
    match base {
        CommitBase::EmptyTree if decoded.parents.is_empty() => Ok((repo.empty_tree(), None)),
        CommitBase::Parent(index) => {
            let id = decoded.parents.get(index).ok_or(CoreError::HistoryBase)?;
            let parent = commit(repo, &id.to_string())?;
            Ok((parent.tree().map_err(git_error)?, Some(id.to_string())))
        }
        _ => Err(CoreError::HistoryBase),
    }
}
/// Lists exact changed leaves relative to a named parent, without rename similarity blob reads.
pub fn read_commit(root: &Path, id: &str, base: CommitBase) -> Result<CommitDetails, CoreError> {
    let repo = open(root)?;
    let root = repo.workdir().ok_or(CoreError::Bare)?.canonicalize()?;
    let commit = commit(&repo, id)?;
    let decoded = commit.decode().map_err(git_error)?;
    let summary = summary(commit.id, &decoded)?;
    let (before, base_id) = base_tree(&repo, &decoded, base)?;
    let after = commit.tree().map_err(git_error)?;
    let mut files = Vec::new();
    let mut limited = false;
    before
        .changes()
        .map_err(git_error)?
        .options(|options| {
            options.track_path().track_rewrites(None);
        })
        .for_each_to_obtain_tree(&after, |change| {
            use gix::object::tree::diff::Change;
            let (kind, old_mode, mode) = match change {
                Change::Addition { entry_mode, .. } => (ChangeKind::Added, None, Some(entry_mode)),
                Change::Deletion { entry_mode, .. } => {
                    (ChangeKind::Deleted, Some(entry_mode), None)
                }
                Change::Modification {
                    previous_entry_mode,
                    entry_mode,
                    ..
                } => (
                    if previous_entry_mode.value() & 0o170000 != entry_mode.value() & 0o170000 {
                        ChangeKind::TypeChanged
                    } else {
                        ChangeKind::Modified
                    },
                    Some(previous_entry_mode),
                    Some(entry_mode),
                ),
                Change::Rewrite { .. } => unreachable!("rewrites explicitly disabled"),
            };
            let old_mode = old_mode.filter(|mode| !mode.is_tree());
            let mode = mode.filter(|mode| !mode.is_tree());
            if old_mode.is_none() && mode.is_none() {
                return Ok(ControlFlow::Continue(()));
            }
            let kind = match (old_mode, mode) {
                (None, Some(_)) => ChangeKind::Added,
                (Some(_), None) => ChangeKind::Deleted,
                _ => kind,
            };
            let path = gix::path::from_bstr(change.location()).into_owned();
            relative_path(&path).map_err(|error| error.raise_erased())?;
            if files.len() == MAX_FILES {
                limited = true;
                return Ok(ControlFlow::Break(()));
            }
            files.push(CommitFile {
                path,
                change: kind,
                old_mode: old_mode.map(|mode| u32::from(mode.value())),
                mode: mode.map(|mode| u32::from(mode.value())),
            });
            Ok(ControlFlow::Continue(()))
        })
        .map_err(git_error)?;
    if limited {
        return Err(CoreError::HistoryLimit);
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(CommitDetails {
        root,
        summary,
        message: utf8_encoding(&decoded)
            .then(|| {
                std::str::from_utf8(decoded.message)
                    .ok()
                    .filter(|text| !text.contains('\0'))
                    .map(str::to_owned)
            })
            .flatten(),
        encoding: decoded
            .encoding
            .map(|name| name.to_str_lossy().into_owned()),
        base,
        base_id,
        files,
    })
}
/// Reads one historical leaf as raw Git bytes/mode, without current attributes or checkout filters.
fn document(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    path: &Path,
) -> Result<Document, CoreError> {
    let Some(entry) = tree.lookup_entry_by_path(path).map_err(git_error)? else {
        return Ok(Document::absent());
    };
    if entry.mode().is_tree() {
        return Ok(Document::absent());
    }
    if entry.mode().is_commit() {
        return Ok(Document::unsupported(Some(u32::from(entry.mode().value()))));
    }
    blob(repo, entry.id().detach(), u32::from(entry.mode().value()))
}
/// Compares only a reviewed changed file, keeping index/worktree and arbitrary external programs untouched.
pub fn read_commit_diff(details: &CommitDetails, path: &Path) -> Result<DiffSnapshot, CoreError> {
    if !details.files.iter().any(|file| file.path == path) {
        return Err(CoreError::InvalidPath);
    }
    let repo = open(&details.root)?;
    relative_path(path)?;
    let commit = commit(&repo, &details.summary.id)?;
    let decoded = commit.decode().map_err(git_error)?;
    let (before, base_id) = base_tree(&repo, &decoded, details.base)?;
    if base_id != details.base_id {
        return Err(CoreError::HistoryBase);
    }
    let after = commit.tree().map_err(git_error)?;
    Ok(DiffSnapshot::new(
        path.to_path_buf(),
        DiffKind::Staged,
        document(&repo, &before, path)?,
        document(&repo, &after, path)?,
    ))
}
