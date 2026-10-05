//! Exact bounded metadata for recovering and guarding externally started Git operations.
use crate::{CommitSnapshot, CoreError, Operation, prepare_commit, repository::open};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

type StateFiles = Vec<(PathBuf, Option<Vec<u8>>)>;

const MAX_METADATA: usize = 4 * 1024 * 1024;
const MAX_STATE_FILES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationState {
    pub kind: Operation,
    pub original_branch: Option<String>,
    pub original_head: Option<String>,
    pub incoming: Vec<String>,
    pub onto: Option<String>,
    pub progress: Option<(usize, usize)>,
    pub interactive: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationSnapshot {
    pub repository: CommitSnapshot,
    pub state: Option<OperationState>,
    files: StateFiles,
    directories: Vec<PathBuf>,
}

/// Reads a regular state file within a shared byte budget; symlinks and devices are explicit refusals.
fn state_file(path: &Path, budget: &mut usize) -> Result<Option<Vec<u8>>, CoreError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || metadata.len() > *budget as u64 {
        return Err(CoreError::OperationMetadata);
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(*budget as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > *budget {
        return Err(CoreError::OperationMetadata);
    }
    *budget -= bytes.len();
    Ok(Some(bytes))
}

/// Captures all flat sequencer/rebase files and known pseudo refs, retaining absence as part of the guard.
fn state_files(repo: &gix::Repository) -> Result<(StateFiles, Vec<PathBuf>), CoreError> {
    let mut budget = MAX_METADATA;
    let mut files = Vec::new();
    let mut directories = Vec::new();
    for name in [
        "MERGE_HEAD",
        "MERGE_MSG",
        "MERGE_MODE",
        "AUTO_MERGE",
        "ORIG_HEAD",
        "REBASE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
    ] {
        files.push((
            name.into(),
            state_file(&repo.git_dir().join(name), &mut budget)?,
        ));
    }
    for name in ["rebase-merge", "rebase-apply", "sequencer"] {
        let directory = repo.git_dir().join(name);
        let metadata = match fs::symlink_metadata(&directory) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_dir() {
            return Err(CoreError::OperationMetadata);
        }
        directories.push(name.into());
        for item in fs::read_dir(directory)? {
            if files.len() >= MAX_STATE_FILES {
                return Err(CoreError::OperationMetadata);
            }
            let relative = PathBuf::from(name).join(item?.file_name());
            files.push((
                relative.clone(),
                state_file(&repo.git_dir().join(relative), &mut budget)?,
            ));
        }
    }
    if directories.contains(&PathBuf::from("rebase-merge"))
        && directories.contains(&PathBuf::from("rebase-apply"))
    {
        return Err(CoreError::OperationMetadata);
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok((files, directories))
}

/// Reads only a requested metadata field; arbitrary messages and patch contents remain private.
fn field<'a>(
    files: &'a [(PathBuf, Option<Vec<u8>>)],
    name: &str,
) -> Result<Option<&'a str>, CoreError> {
    files
        .iter()
        .find(|(path, _)| path == Path::new(name))
        .and_then(|(_, bytes)| bytes.as_deref())
        .map(|bytes| {
            std::str::from_utf8(bytes)
                .map(|text| text.trim_end_matches(['\n', '\r']))
                .map_err(|_| CoreError::OperationMetadata)
        })
        .transpose()
}

/// Validates an operation object id against the selected repository's actual hash format.
fn object_id(value: Option<&str>, repo: &gix::Repository) -> Result<Option<String>, CoreError> {
    value
        .map(|value| {
            let oid = gix::hash::ObjectId::from_hex(value.as_bytes())
                .map_err(|_| CoreError::OperationMetadata)?;
            if oid.kind() != repo.object_hash() {
                return Err(CoreError::OperationMetadata);
            }
            Ok(oid.to_string())
        })
        .transpose()
}

/// Extracts explicit merge/rebase provenance and progress without interpreting arbitrary patch text.
fn describe(
    repo: &gix::Repository,
    files: &[(PathBuf, Option<Vec<u8>>)],
) -> Result<Option<OperationState>, CoreError> {
    let Some(operation) = repo.state() else {
        return Ok(None);
    };
    let mut state = OperationState {
        kind: Operation::Other,
        original_branch: None,
        original_head: None,
        incoming: Vec::new(),
        onto: None,
        progress: None,
        interactive: false,
    };
    match operation {
        gix::state::InProgress::Merge => {
            state.kind = Operation::Merge;
            let incoming = field(files, "MERGE_HEAD")?.ok_or(CoreError::OperationMetadata)?;
            for line in incoming.lines() {
                state
                    .incoming
                    .push(object_id(Some(line), repo)?.ok_or(CoreError::OperationMetadata)?);
            }
            if state.incoming.is_empty() {
                return Err(CoreError::OperationMetadata);
            }
            state.original_head = object_id(field(files, "ORIG_HEAD")?, repo)?;
        }
        gix::state::InProgress::Rebase
        | gix::state::InProgress::RebaseInteractive
        | gix::state::InProgress::ApplyMailboxRebase => {
            state.kind = Operation::Rebase;
            state.interactive = operation == gix::state::InProgress::RebaseInteractive;
            let directory = if repo.git_dir().join("rebase-merge").is_dir() {
                "rebase-merge"
            } else {
                "rebase-apply"
            };
            let get = |name| field(files, &format!("{directory}/{name}"));
            let branch = get("head-name")?.ok_or(CoreError::OperationMetadata)?;
            state.original_branch = if branch == "detached HEAD" {
                None
            } else {
                let name = branch
                    .strip_prefix("refs/heads/")
                    .ok_or(CoreError::OperationMetadata)?;
                crate::branches::branch_name(name).map_err(|_| CoreError::OperationMetadata)?;
                Some(name.to_owned())
            };
            state.original_head = object_id(get("orig-head")?.or(get("head")?), repo)?;
            state.onto = object_id(get("onto")?, repo)?;
            if state.original_head.is_none() || state.onto.is_none() {
                return Err(CoreError::OperationMetadata);
            }
            if let Some(incoming) = object_id(
                field(files, "REBASE_HEAD")?.or(get("original-commit")?),
                repo,
            )? {
                state.incoming.push(incoming);
            }
            let (current, total) = if directory == "rebase-merge" {
                (get("msgnum")?, get("end")?)
            } else {
                (get("next")?, get("last")?)
            };
            if let (Some(current), Some(total)) = (current, total) {
                let current: usize = current.parse().map_err(|_| CoreError::OperationMetadata)?;
                let total: usize = total.parse().map_err(|_| CoreError::OperationMetadata)?;
                if total == 0 || current == 0 || current > total {
                    return Err(CoreError::OperationMetadata);
                }
                state.progress = Some((current, total));
            }
        }
        _ => {}
    }
    Ok(Some(state))
}

/// Recovers current state on a fresh read and rejects a partially observed transition between sequencer stops.
pub fn prepare_operation(root: &Path) -> Result<OperationSnapshot, CoreError> {
    let repository = prepare_commit(root)?;
    let repo = open(&repository.root)?;
    let (files, directories) = if repo.state().is_some() {
        state_files(&repo)?
    } else {
        (Vec::new(), Vec::new())
    };
    let state = describe(&repo, &files)?;
    if prepare_commit(&repository.root)? != repository {
        return Err(CoreError::Stale);
    }
    if state.is_some()
        && state_files(&open(&repository.root)?)? != (files.clone(), directories.clone())
    {
        return Err(CoreError::Stale);
    }
    Ok(OperationSnapshot {
        repository,
        state,
        files,
        directories,
    })
}
