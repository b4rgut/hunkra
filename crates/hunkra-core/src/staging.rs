//! Exact snapshot selection and atomic stage-zero index updates through official gix APIs.
use crate::{
    CoreError, DiffKind, DiffSnapshot, Document, IndexLimitation, Line, Presentation, read_diff,
    repository::{capture_guard, git_error, open},
};
use gix::bstr::ByteSlice;
use std::{collections::BTreeSet, fs, io::Write, path::Path};

#[derive(Clone, Debug, Default)]
pub struct LineSelection {
    pub removed: BTreeSet<usize>,
    pub added: BTreeSet<usize>,
}

#[derive(Clone, Debug)]
pub enum Selection {
    File,
    Lines(LineSelection),
}

#[derive(Debug)]
pub struct IndexWrite {
    pub backend: &'static str,
}

/// Reconstitutes exact UTF-8 line bytes, preserving its original newline or lack thereof.
fn append(out: &mut Vec<u8>, line: &Line) -> Result<(), CoreError> {
    if !out.is_empty() && !out.ends_with(b"\n") {
        // Independent halves of an EOF replacement must not silently concatenate two lines.
        return Err(CoreError::SelectionBoundary);
    }
    out.extend(line.text.as_bytes());
    out.extend(match line.ending {
        "LF" => b"\n".as_slice(),
        "CRLF" => b"\r\n".as_slice(),
        _ => b"".as_slice(),
    });
    Ok(())
}

/// Builds the chosen index version; +/- selections are independent, including replacements.
fn selected_document(diff: &DiffSnapshot, selection: &Selection) -> Result<Document, CoreError> {
    if matches!(
        diff.presentation,
        Presentation::Conflict | Presentation::Unsupported | Presentation::TooLarge
    ) {
        return Err(CoreError::UnsupportedWrite);
    }
    if matches!(selection, Selection::File) {
        return Ok(match diff.kind {
            DiffKind::Unstaged => diff.right.clone(),
            DiffKind::Staged => diff.left.clone(),
        });
    }
    if diff.presentation != Presentation::Text {
        return Err(CoreError::UnsupportedWrite);
    }
    let Selection::Lines(selection) = selection else {
        unreachable!()
    };
    if selection.removed.is_empty() && selection.added.is_empty() {
        return Err(CoreError::EmptySelection);
    }
    let mut removed = BTreeSet::new();
    let mut added = BTreeSet::new();
    for row in diff.rows.iter().filter(|row| row.changed) {
        if let Some(line) = &row.left {
            removed.insert(line.number - 1);
        }
        if let Some(line) = &row.right {
            added.insert(line.number - 1);
        }
    }
    if !selection.removed.is_subset(&removed) || !selection.added.is_subset(&added) {
        return Err(CoreError::Stale);
    }
    let mut bytes = Vec::new();
    for row in &diff.rows {
        if !row.changed {
            if let Some(line) = &row.left {
                append(&mut bytes, line)?;
            }
            continue;
        }
        let is_staging = diff.kind == DiffKind::Unstaged;
        if let Some(line) = &row.left {
            let chosen = selection.removed.contains(&(line.number - 1));
            if chosen != is_staging {
                append(&mut bytes, line)?;
            }
        }
        if let Some(line) = &row.right {
            let chosen = selection.added.contains(&(line.number - 1));
            if chosen == is_staging {
                append(&mut bytes, line)?;
            }
        }
    }
    // Selecting every deletion from a deleted file means the entry is absent, not an empty blob.
    let target = if diff.kind == DiffKind::Unstaged {
        &diff.right
    } else {
        &diff.left
    };
    let base = if diff.kind == DiffKind::Unstaged {
        &diff.left
    } else {
        &diff.right
    };
    if !target.present && bytes.is_empty() {
        return Ok(Document::absent());
    }
    if !base.present && bytes.is_empty() {
        return Ok(Document::absent());
    }
    let mode = base.mode.or(target.mode);
    if mode.is_some_and(|mode| !matches!(mode, 0o100644 | 0o100755)) {
        return Err(CoreError::UnsupportedWrite);
    }
    Ok(Document::present(bytes, mode))
}

/// Refuses actual index/hook variants until their semantics are tested, before any write.
pub(crate) fn supported(
    repo: &gix::Repository,
    index: &gix::index::State,
    path: &Path,
) -> Result<(), CoreError> {
    let filesystem = repo.filesystem_options().map_err(git_error)?;
    if filesystem.ignore_case {
        return Err(CoreError::IndexVariant(IndexLimitation::CaseFolding));
    }
    if filesystem.precompose_unicode {
        return Err(CoreError::IndexVariant(IndexLimitation::UnicodeComposition));
    }
    if index.is_sparse() {
        return Err(CoreError::IndexVariant(IndexLimitation::Sparse));
    }
    if index.link().is_some() {
        return Err(CoreError::IndexVariant(IndexLimitation::Split));
    }
    // File::at dissolves split indexes and removes LINK; decode the original state for preflight.
    match fs::read(repo.index_path()) {
        Ok(bytes) => {
            let metadata = fs::metadata(repo.index_path())?;
            let (original, _) = gix::index::State::from_bytes(
                &bytes,
                metadata.modified()?.into(),
                repo.object_hash(),
                Default::default(),
            )
            .map_err(git_error)?;
            if original.link().is_some() {
                return Err(CoreError::IndexVariant(IndexLimitation::Split));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let bytes = gix::path::into_bstr(path);
    if let Some(entry) = index.entry_by_path(bytes.as_bstr())
        && entry
            .flags
            .contains(gix::index::entry::Flags::SKIP_WORKTREE)
    {
        return Err(CoreError::IndexVariant(IndexLimitation::SkipWorktree));
    }
    // A file/directory collision needs a separate multi-path transaction, not a malformed index.
    let requested = bytes.as_bstr().as_bytes();
    if index.entries().iter().any(|entry| {
        let existing = entry.path(index).as_bytes();
        existing != requested
            && ((existing.starts_with(requested) && existing.get(requested.len()) == Some(&b'/'))
                || (requested.starts_with(existing)
                    && requested.get(existing.len()) == Some(&b'/')))
    }) {
        return Err(CoreError::IndexVariant(IndexLimitation::PathCollision));
    }
    let config = repo.config_snapshot();
    if config
        .string("core.sharedRepository")
        .is_some_and(|value| !matches!(value.as_bstr().as_bytes(), b"0" | b"false"))
    {
        return Err(CoreError::IndexVariant(IndexLimitation::SharedPermissions));
    }
    let hooks = config
        .trusted_path("core.hooksPath")
        .map_err(git_error)?
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                repo.workdir().expect("working repository").join(path)
            }
        })
        .unwrap_or_else(|| repo.common_dir().join("hooks"));
    if let Some(metadata) = match fs::metadata(hooks.join("post-index-change")) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    } {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 != 0 {
                return Err(CoreError::IndexVariant(IndexLimitation::IndexHook));
            }
        }
        #[cfg(not(unix))]
        return Err(CoreError::IndexVariant(IndexLimitation::IndexHook));
    }
    if fs::symlink_metadata(repo.index_path()).is_ok_and(|metadata| metadata.is_symlink()) {
        return Err(CoreError::IndexVariant(IndexLimitation::Symlink));
    }
    Ok(())
}

/// Updates only the selected index path under index.lock, never writing the working file.
pub fn update_index(
    root: &Path,
    diff: &DiffSnapshot,
    selection: &Selection,
) -> Result<IndexWrite, CoreError> {
    let desired = selected_document(diff, selection)?;
    let expected = diff.guard.as_ref().ok_or(CoreError::Stale)?;
    let repo = open(root)?;
    let root = repo.workdir().ok_or(CoreError::Bare)?.canonicalize()?;
    if root != expected.root {
        return Err(CoreError::Stale);
    }
    let index_before = repo.index_or_empty().map_err(git_error)?;
    supported(&repo, &index_before, &diff.path)?;
    let lock_path = repo.index_path();
    let permissions = match fs::metadata(&lock_path) {
        Ok(metadata) => Some(metadata.permissions()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let lock = match permissions {
        Some(permissions) => gix::lock::File::acquire_to_update_resource_with_permissions(
            &lock_path,
            gix::lock::acquire::Fail::Immediately,
            None,
            || permissions.clone(),
        ),
        None => gix::lock::File::acquire_to_update_resource(
            &lock_path,
            gix::lock::acquire::Fail::Immediately,
            None,
        ),
    };
    let mut lock = lock.map_err(|_| CoreError::IndexLock)?;
    // A fresh repository/index is loaded after locking, not the cached pre-lock index.
    let fresh = open(&root)?;
    let current = read_diff(&root, &diff.path, diff.kind)?;
    if !diff.same_version(&current) {
        return Err(CoreError::Stale);
    }
    let mut index = (*fresh.index_or_empty().map_err(git_error)?).clone();
    supported(&fresh, &index, &diff.path)?;
    let preserved = match fs::read(fresh.index_path()) {
        Ok(bytes) => crate::index_format::preserve(&bytes, fresh.object_hash())?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    let bytes = gix::path::into_bstr(&diff.path);
    index.remove_entries(|_, path, _| path == bytes.as_bstr());
    if desired.present {
        let id = fresh
            .write_blob(&desired.bytes)
            .map_err(git_error)?
            .detach();
        let mode = gix::index::entry::Mode::from_bits(desired.mode.unwrap_or(0o100644))
            .ok_or(CoreError::UnsupportedWrite)?;
        index.dangerously_push_entry(
            Default::default(),
            id,
            gix::index::entry::Flags::empty(),
            mode,
            bytes.as_bstr(),
        );
        index.sort_entries();
    }
    // A valid but stale TREE cache otherwise causes Git commit/write-tree to lose staged content.
    index.remove_tree();
    crate::index_format::write(&index, &preserved, &mut lock)?;
    if let Ok(metadata) = fs::metadata(&lock_path) {
        lock.with_mut(|file| file.set_permissions(metadata.permissions()))?;
    }
    lock.flush()?;
    lock.with_mut(|file| file.sync_all())?;
    if capture_guard(&fresh, &root, &diff.path)? != *expected {
        return Err(CoreError::Stale);
    }
    lock.commit().map_err(git_error)?;
    Ok(IndexWrite { backend: "gix" })
}
