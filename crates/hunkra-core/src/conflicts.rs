//! Actual index stages, worktree Result persistence and separate stage-zero resolution.
use crate::{
    CoreError, DiffKind, DiffSnapshot, Document, OperationSnapshot, Presentation,
    diff::MAX_BYTES,
    index_format::UndoStages,
    prepare_operation,
    repository::{RepositoryGuard, at_worktree, blob, capture_guard, check_path, git_error, open},
};
use gix::bstr::ByteSlice;
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictSide {
    pub id: Option<String>,
    /// Original blob bytes, before attributes and checkout filters.
    pub git: Document,
    /// Side in the working file's representation, suitable for explicit editor choices.
    pub worktree: Document,
}

#[derive(Clone, Debug)]
pub struct ConflictSnapshot {
    pub path: PathBuf,
    pub base: ConflictSide,
    /// Stage 2: current HEAD/upstream when replaying a commit during rebase.
    pub a: ConflictSide,
    /// Stage 3: incoming merge side/replayed commit during rebase.
    pub b: ConflictSide,
    pub result: Document,
    pub presentation: Presentation,
    pub resolved: bool,
    pub operation: OperationSnapshot,
    guard: RepositoryGuard,
    stages: UndoStages,
}

#[derive(Debug)]
pub struct ConflictWrite {
    pub backend: &'static str,
}

impl ConflictSnapshot {
    /// Chooses one current Result block while preserving the actual sides' terminal line endings.
    pub fn choose_chunk(
        &self,
        bytes: &[u8],
        index: usize,
        choice: crate::conflict_chunks::ChunkChoice,
    ) -> Result<Vec<u8>, CoreError> {
        if self.presentation != Presentation::Text {
            return Err(CoreError::ConflictUnsupported);
        }
        crate::conflict_chunks::choose_with_sides(
            bytes,
            index,
            choice,
            &self.a.worktree.bytes,
            &self.b.worktree.bytes,
        )
    }
}

/// Refuses symlink parents and metadata paths before a working file is replaced or removed.
fn working_path(root: &Path, path: &Path) -> Result<PathBuf, CoreError> {
    check_path(root, path)?;
    let mut current = root.to_path_buf();
    let count = path.components().count();
    for (offset, component) in path.components().enumerate() {
        let Component::Normal(name) = component else {
            return Err(CoreError::InvalidPath);
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_symlink() => return Err(CoreError::ConflictUnsupported),
            Ok(metadata) if offset + 1 < count && !metadata.is_dir() => {
                return Err(CoreError::InvalidPath);
            }
            Ok(metadata) if offset + 1 == count && !metadata.is_file() => {
                return Err(CoreError::ConflictUnsupported);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(current)
}

/// Converts one bounded regular blob through gix's checkout filters; unknown encodings fail.
fn side(
    repo: &gix::Repository,
    path: &Path,
    stage: Option<(u32, gix::ObjectId)>,
) -> Result<ConflictSide, CoreError> {
    let Some((mode, id)) = stage else {
        return Ok(ConflictSide {
            id: None,
            git: Document::absent(),
            worktree: Document::absent(),
        });
    };
    let git = blob(repo, id, mode)?;
    let worktree = if git.limited || git.unsupported || !matches!(mode, 0o100644 | 0o100755) {
        git.clone()
    } else if unsupported_checkout_encoding(repo, path)? {
        // encoding_rs's encoder emits UTF-8 for UTF-16 labels; never advertise those bytes as a checkout.
        Document::unsupported(Some(mode))
    } else {
        let (mut pipeline, _) = repo.filter_pipeline(None).map_err(git_error)?;
        let bytes = gix::path::into_bstr(path);
        let converted = pipeline.convert_to_worktree(&git.bytes, bytes.as_bstr(),
            gix::filter::plumbing::pipeline::convert::to_worktree::Options {
                can_delay: gix::filter::plumbing::driver::apply::Delay::Forbid,
                unknown_encoding: gix::filter::plumbing::pipeline::convert::to_worktree::UnknownEncoding::Fail,
            }).map_err(git_error)?;
        let mut result = Vec::new();
        converted
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut result)?;
        if result.len() > MAX_BYTES {
            Document::oversized(result.len() as u64, Some(mode))
        } else {
            Document::present(result, Some(mode))
        }
    };
    Ok(ConflictSide {
        id: Some(id.to_string()),
        git,
        worktree,
    })
}

/// Audits the actual encoding attribute before using the current gix checkout primitive.
fn unsupported_checkout_encoding(repo: &gix::Repository, path: &Path) -> Result<bool, CoreError> {
    let index = repo.index_or_empty().map_err(git_error)?;
    let mut attributes = repo
        .attributes_only(
            &index,
            gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
        )
        .map_err(git_error)?;
    let mut matches = attributes.selected_attribute_matches(["working-tree-encoding"]);
    attributes
        .at_path(path, None)
        .map_err(git_error)?
        .matching_attributes(&mut matches);
    Ok(matches.iter_selected().any(|attribute| {
        if let gix::attrs::StateRef::Value(name) = attribute.assignment.state {
            let encoding =
                gix::filter::plumbing::encoding::Encoding::for_label(name.as_bstr().as_bytes());
            encoding.is_some_and(|encoding| {
                encoding == gix::filter::plumbing::encoding::UTF_16LE
                    || encoding == gix::filter::plumbing::encoding::UTF_16BE
            })
        } else {
            false
        }
    }))
}

/// Reads an unresolved stage set, or historical REUC sources only during an active operation.
pub fn read_conflict(root: &Path, path: &Path) -> Result<ConflictSnapshot, CoreError> {
    let repo = open(root)?;
    let root = repo.workdir().ok_or(CoreError::Bare)?.canonicalize()?;
    check_path(&root, path)?;
    let operation = prepare_operation(&root)?;
    let guard = capture_guard(&repo, &root, path)?;
    let index = repo.index_or_empty().map_err(git_error)?;
    let bytes = gix::path::into_bstr(path);
    let mut stages = [None; 3];
    for entry in index
        .entries()
        .iter()
        .filter(|entry| entry.path(&index) == bytes.as_bstr())
    {
        let stage = entry.stage() as usize;
        if stage != 0 {
            stages[stage - 1] = Some((entry.mode.bits(), entry.id));
        }
    }
    let resolved = stages.iter().all(Option::is_none);
    if resolved {
        if operation.state.is_none() {
            return Err(CoreError::NoConflict);
        }
        stages = crate::index_format::undo_stages(
            &fs::read(repo.index_path())?,
            repo.object_hash(),
            bytes.as_bytes(),
        )?
        .ok_or(CoreError::NoConflict)?;
    }
    let base = side(&repo, path, stages[0])?;
    let a = side(&repo, path, stages[1])?;
    let b = side(&repo, path, stages[2])?;
    let result = match (&guard.raw_worktree, guard.mode) {
        (Some(raw), mode) if raw.len() > MAX_BYTES => Document::oversized(raw.len() as u64, mode),
        (Some(raw), mode) => Document::present(raw.clone(), mode),
        (None, None) => Document::absent(),
        (None, mode) => Document::unsupported(mode),
    };
    let mut presentation = Presentation::Text;
    for document in [&base.worktree, &a.worktree, &b.worktree, &result] {
        let candidate = DiffSnapshot::new(
            path.to_path_buf(),
            DiffKind::Unstaged,
            Document::absent(),
            document.clone(),
        )
        .presentation;
        if document
            .mode
            .is_some_and(|mode| !matches!(mode, 0o100644 | 0o100755))
        {
            presentation = Presentation::Unsupported;
            break;
        }
        if candidate != Presentation::Text {
            presentation = if document.unsupported
                && !base.git.unsupported
                && !a.git.unsupported
                && !b.git.unsupported
            {
                Presentation::InvalidEncoding
            } else {
                candidate
            };
            break;
        }
    }
    if capture_guard(&repo, &root, path)? != guard || prepare_operation(&root)? != operation {
        return Err(CoreError::Stale);
    }
    Ok(ConflictSnapshot {
        path: path.to_path_buf(),
        base,
        a,
        b,
        result,
        presentation,
        resolved,
        operation,
        guard,
        stages,
    })
}

/// Checks exact worktree/index/HEAD and operation metadata without using a cached index.
fn verify(snapshot: &ConflictSnapshot, repo: &gix::Repository) -> Result<(), CoreError> {
    let root = &snapshot.operation.repository.root;
    if capture_guard(repo, root, &snapshot.path)? != snapshot.guard
        || prepare_operation(root)? != snapshot.operation
    {
        return Err(CoreError::Stale);
    }
    let current = read_conflict(root, &snapshot.path)?;
    if current.base != snapshot.base
        || current.a != snapshot.a
        || current.b != snapshot.b
        || current.result != snapshot.result
        || current.presentation != snapshot.presentation
        || current.resolved != snapshot.resolved
    {
        return Err(CoreError::Stale);
    }
    Ok(())
}

/// Saves only Result bytes or explicit deletion; it never stages, resolves or continues Git.
pub fn save_result(
    snapshot: &ConflictSnapshot,
    desired: &Document,
) -> Result<ConflictWrite, CoreError> {
    if snapshot.presentation != Presentation::Text
        || desired.limited
        || desired.unsupported
        || desired.bytes.len() > MAX_BYTES
        || desired.bytes.contains(&0)
        || std::str::from_utf8(&desired.bytes).is_err()
        || desired
            .mode
            .is_some_and(|mode| !matches!(mode, 0o100644 | 0o100755))
        || (!desired.present && !desired.bytes.is_empty())
    {
        return Err(CoreError::ConflictUnsupported);
    }
    let root = &snapshot.operation.repository.root;
    let repo = open(root)?;
    let path = working_path(root, &snapshot.path)?;
    let index = repo.index_or_empty().map_err(git_error)?;
    crate::staging::supported(&repo, &index, &snapshot.path)?;
    let _index_lock = gix::lock::Marker::acquire_to_hold_resource(
        repo.index_path(),
        gix::lock::acquire::Fail::Immediately,
        None,
    )
    .map_err(|_| CoreError::IndexLock)?;
    verify(snapshot, &repo)?;
    if !desired.present {
        let _file_lock = gix::lock::Marker::acquire_to_hold_resource(
            &path,
            gix::lock::acquire::Fail::Immediately,
            None,
        )
        .map_err(|_| CoreError::WorktreeLock)?;
        verify(snapshot, &repo)?;
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    } else {
        let permissions = fs::metadata(&path)
            .ok()
            .map(|metadata| metadata.permissions());
        let lock = if let Some(permissions) = &permissions {
            gix::lock::File::acquire_to_update_resource_with_permissions(
                &path,
                gix::lock::acquire::Fail::Immediately,
                Some(root.clone()),
                || permissions.clone(),
            )
        } else {
            gix::lock::File::acquire_to_update_resource(
                &path,
                gix::lock::acquire::Fail::Immediately,
                Some(root.clone()),
            )
        };
        let mut lock = lock.map_err(|_| CoreError::WorktreeLock)?;
        lock.write_all(&desired.bytes)?;
        #[cfg(unix)]
        if let Some(mode) = desired.mode {
            use std::os::unix::fs::PermissionsExt;
            lock.with_mut(|file| {
                let previous = file.metadata()?.permissions().mode();
                file.set_permissions(fs::Permissions::from_mode(
                    (previous & !0o111) | if mode == 0o100755 { 0o111 } else { 0 },
                ))
            })?;
        }
        lock.flush()?;
        lock.with_mut(|file| file.sync_all())?;
        working_path(root, &snapshot.path)?;
        verify(snapshot, &repo)?;
        lock.commit().map_err(git_error)?;
    }
    Ok(ConflictWrite {
        backend: "filesystem",
    })
}

/// Explicitly records the current working file/deletion in stage zero and its former stages in REUC.
pub fn mark_resolved(snapshot: &ConflictSnapshot) -> Result<ConflictWrite, CoreError> {
    if snapshot.resolved {
        return Err(CoreError::NoConflict);
    }
    if snapshot
        .stages
        .iter()
        .flatten()
        .any(|(mode, _)| !matches!(mode, 0o100644 | 0o100755))
    {
        return Err(CoreError::ConflictUnsupported);
    }
    let root = &snapshot.operation.repository.root;
    let repo = open(root)?;
    working_path(root, &snapshot.path)?;
    let initial = repo.index_or_empty().map_err(git_error)?;
    crate::staging::supported(&repo, &initial, &snapshot.path)?;
    let permissions = fs::metadata(repo.index_path())?.permissions();
    let mut lock = gix::lock::File::acquire_to_update_resource_with_permissions(
        repo.index_path(),
        gix::lock::acquire::Fail::Immediately,
        None,
        || permissions.clone(),
    )
    .map_err(|_| CoreError::IndexLock)?;
    let fresh = open(root)?;
    verify(snapshot, &fresh)?;
    let mut index = (*fresh.index_or_empty().map_err(git_error)?).clone();
    crate::staging::supported(&fresh, &index, &snapshot.path)?;
    let raw = fs::read(fresh.index_path())?;
    let preserved = crate::index_format::preserve(&raw, fresh.object_hash())?;
    let bytes = gix::path::into_bstr(&snapshot.path);
    let preserved = crate::index_format::with_resolve_undo(
        &preserved,
        fresh.object_hash(),
        bytes.as_bytes(),
        snapshot.stages,
    )?;
    let desired = at_worktree(&fresh, &index, root, &snapshot.path)?;
    if desired.limited
        || desired.unsupported
        || desired
            .mode
            .is_some_and(|mode| !matches!(mode, 0o100644 | 0o100755))
    {
        return Err(CoreError::ConflictUnsupported);
    }
    index.remove_entries(|_, path, _| path == bytes.as_bstr());
    if desired.present {
        let id = fresh
            .write_blob(&desired.bytes)
            .map_err(git_error)?
            .detach();
        let mode = gix::index::entry::Mode::from_bits(desired.mode.unwrap_or(0o100644))
            .ok_or(CoreError::ConflictUnsupported)?;
        index.dangerously_push_entry(
            Default::default(),
            id,
            gix::index::entry::Flags::empty(),
            mode,
            bytes.as_bstr(),
        );
        index.sort_entries();
    }
    index.remove_tree();
    crate::index_format::write(&index, &preserved, &mut lock)?;
    lock.flush()?;
    lock.with_mut(|file| file.set_permissions(permissions.clone()))?;
    lock.with_mut(|file| file.sync_all())?;
    working_path(root, &snapshot.path)?;
    verify(snapshot, &fresh)?;
    lock.commit().map_err(git_error)?;
    Ok(ConflictWrite { backend: "gix" })
}
