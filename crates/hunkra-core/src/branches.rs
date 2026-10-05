//! Read-only branch enumeration and guarded local reference creation through gix.
use crate::{
    CommitSnapshot, CoreError,
    commit::hook,
    prepare_commit,
    repository::{git_error, open},
};
use gix::{
    bstr::ByteSlice,
    refs::transaction::{PreviousValue, RefEdit},
};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub id: String,
}
#[derive(Clone, Debug)]
pub struct BranchSnapshot {
    pub repository: CommitSnapshot,
    pub branches: Vec<Branch>,
}
#[derive(Debug)]
pub struct BranchWrite {
    pub branch: Branch,
    pub backend: &'static str,
    pub reference_warning: bool,
    pub post_hook_failed: bool,
}

/// Reads loose/packed local branches without writing index stats, objects, refs or configuration.
pub fn read_branches(root: &Path) -> Result<BranchSnapshot, CoreError> {
    let snapshot = prepare_commit(root)?;
    let repo = open(&snapshot.root)?;
    // Peeling can replace a symbolic reference's name; preserve its advertised name first.
    // Drop the iterator before peeling so the packed-reference buffer is not held recursively.
    let names = {
        let platform = repo.references().map_err(git_error)?;
        let mut names = Vec::new();
        for reference in platform.local_branches().map_err(git_error)? {
            let reference = reference.map_err(git_error)?;
            if names.len() >= 10_000 {
                return Err(CoreError::ReferenceLimit);
            }
            let name = std::str::from_utf8(reference.name().as_bstr())
                .map_err(|_| CoreError::ReferenceLimit)?;
            names.push(name.to_owned());
        }
        names
    };
    let mut branches = Vec::with_capacity(names.len());
    for full in names {
        let id = repo
            .find_reference(full.as_str())
            .map_err(git_error)?
            .peel_to_id()
            .map_err(git_error)?
            .to_string();
        let name = full
            .strip_prefix("refs/heads/")
            .ok_or(CoreError::ReferenceLimit)?
            .to_owned();
        branches.push(Branch { name, id });
    }
    branches.sort_by(|a, b| a.name.cmp(&b.name));
    if prepare_commit(&snapshot.root)? != snapshot {
        return Err(CoreError::Stale);
    }
    Ok(BranchSnapshot {
        repository: snapshot,
        branches,
    })
}

/// Validates branch-specific Git rules and rejects option-like input without rewriting the user's name.
pub(crate) fn branch_name(name: &str) -> Result<String, CoreError> {
    if name.is_empty() || name.len() > 4096 || name.starts_with('-') {
        return Err(CoreError::BranchName);
    }
    let full = format!("refs/heads/{name}");
    gix::validate::reference::branch_name(full.as_bytes().as_bstr())
        .map_err(|_| CoreError::BranchName)?;
    Ok(full)
}

/// Creates only the requested absent branch from the reviewed HEAD; dirty working files are never touched.
pub fn create_branch(snapshot: &CommitSnapshot, name: &str) -> Result<BranchWrite, CoreError> {
    let full = branch_name(name)?;
    if prepare_commit(&snapshot.root)? != *snapshot {
        return Err(CoreError::Stale);
    }
    let repo = open(&snapshot.root)?;
    if repo.state().is_some() {
        return Err(CoreError::ActiveOperation);
    }
    match repo.find_reference(full.as_str()) {
        Ok(_) => return Err(CoreError::BranchExists),
        Err(error) if error.is_not_found() => {}
        Err(error) => return Err(git_error(error)),
    }
    let id = snapshot.head.as_ref().ok_or(CoreError::Unborn)?;
    let id = gix::hash::ObjectId::from_hex(id.as_bytes()).map_err(git_error)?;
    repo.find_commit(id).map_err(git_error)?;
    let committer = repo
        .committer()
        .transpose()
        .map_err(|_| CoreError::Identity)?
        .ok_or(CoreError::Identity)?;
    let index_lock = gix::lock::Marker::acquire_to_hold_resource(
        repo.index_path(),
        gix::lock::acquire::Fail::Immediately,
        None,
    )
    .map_err(|_| CoreError::IndexLock)?;
    let head_lock = gix::lock::Marker::acquire_to_hold_resource(
        repo.git_dir().join("HEAD"),
        gix::lock::acquire::Fail::Immediately,
        None,
    )
    .map_err(|_| CoreError::ReferenceUpdate)?;
    if prepare_commit(&snapshot.root)? != *snapshot {
        return Err(CoreError::Stale);
    }
    let edit = RefEdit::update(
        full.as_str().try_into().map_err(git_error)?,
        id,
        PreviousValue::MustNotExist,
        "branch: Created from HEAD",
    );
    let input = format!("{} {id} {full}\n", repo.object_hash().null());
    hook(
        &repo,
        "reference-transaction",
        &[Path::new("preparing")],
        Some(input.as_bytes()),
    )?;
    let transaction = repo
        .refs
        .transaction()
        .prepare(
            [edit],
            gix::lock::acquire::Fail::Immediately,
            gix::lock::acquire::Fail::Immediately,
        )
        .map_err(|_| {
            if open(&snapshot.root).is_ok_and(|repo| repo.find_reference(full.as_str()).is_ok()) {
                CoreError::BranchExists
            } else {
                CoreError::ReferenceUpdate
            }
        })?;
    hook(
        &repo,
        "reference-transaction",
        &[Path::new("prepared")],
        Some(input.as_bytes()),
    )?;
    if prepare_commit(&snapshot.root)? != *snapshot {
        drop(transaction);
        drop(head_lock);
        drop(index_lock);
        let _ = hook(
            &repo,
            "reference-transaction",
            &[Path::new("aborted")],
            Some(input.as_bytes()),
        );
        return Err(CoreError::Stale);
    }
    let reference_warning = transaction.commit(Some(committer)).is_err();
    if reference_warning
        && open(&snapshot.root)?
            .find_reference(full.as_str())
            .map_err(git_error)?
            .try_id()
            .map(|id| id.to_string())
            .as_deref()
            != Some(&id.to_string())
    {
        return Err(CoreError::ReferenceUpdate);
    }
    drop(head_lock);
    drop(index_lock);
    let post_hook_failed = hook(
        &repo,
        "reference-transaction",
        &[Path::new("committed")],
        Some(input.as_bytes()),
    )
    .is_err();
    Ok(BranchWrite {
        branch: Branch {
            name: name.to_owned(),
            id: id.to_string(),
        },
        backend: "gix",
        reference_warning,
        post_hook_failed,
    })
}
