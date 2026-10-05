//! Explicit preflight CLI routing only for audited worktree/sequencer coordinator gaps.
use crate::{
    Branch, CoreError, Operation, OperationSnapshot, OperationState,
    branches::branch_name,
    prepare_operation,
    repository::{git_error, open},
};
use std::{io::Read, process::Stdio};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OperationAction {
    Switch(Branch),
    Merge(Branch),
    Rebase(Branch),
    Continue,
    Abort,
    Skip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationCapability {
    WorktreeSwitchCoordinator,
    MergeRepositoryCoordinator,
    RebaseSequencerCoordinator,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperationRoute {
    pub backend: &'static str,
    pub capability: OperationCapability,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationOutcome {
    Completed,
    StoppedWithConflicts,
    StoppedForReview,
}

#[derive(Debug)]
pub struct OperationWrite {
    pub route: OperationRoute,
    pub outcome: OperationOutcome,
    pub head: Option<String>,
    pub state: Option<OperationState>,
    pub diagnostic_bytes: usize,
}

/// Selects one executor before any write; gix remains the executor for reads/ref creation/index/ordinary Commit.
pub fn operation_route(
    snapshot: &OperationSnapshot,
    action: &OperationAction,
) -> Result<OperationRoute, CoreError> {
    let capability = match action {
        OperationAction::Switch(_) => OperationCapability::WorktreeSwitchCoordinator,
        OperationAction::Merge(_) => OperationCapability::MergeRepositoryCoordinator,
        OperationAction::Rebase(_) => OperationCapability::RebaseSequencerCoordinator,
        OperationAction::Continue | OperationAction::Abort | OperationAction::Skip => {
            match snapshot.state.as_ref().ok_or(CoreError::NoOperation)?.kind {
                Operation::Merge if *action != OperationAction::Skip => {
                    OperationCapability::MergeRepositoryCoordinator
                }
                Operation::Rebase => OperationCapability::RebaseSequencerCoordinator,
                _ => return Err(CoreError::OperationAction),
            }
        }
    };
    Ok(OperationRoute {
        backend: "git-cli",
        capability,
    })
}

/// Validates the advertised branch and its immutable commit target, including symbolic references.
fn target(repo: &gix::Repository, branch: &Branch) -> Result<(), CoreError> {
    let full = branch_name(&branch.name)?;
    let mut reference = repo.find_reference(full.as_str()).map_err(|error| {
        if error.is_not_found() {
            CoreError::BranchMissing
        } else {
            git_error(error)
        }
    })?;
    let id = reference.peel_to_id().map_err(git_error)?;
    let expected =
        gix::hash::ObjectId::from_hex(branch.id.as_bytes()).map_err(|_| CoreError::Stale)?;
    if id.detach() != expected {
        return Err(CoreError::Stale);
    }
    repo.find_commit(id).map_err(git_error)?;
    Ok(())
}

/// Drains child diagnostics using constant memory; arbitrary hook/config output is never retained or surfaced.
fn drain(mut pipe: impl Read) -> std::io::Result<usize> {
    let mut total = 0usize;
    let mut buffer = [0u8; 8192];
    loop {
        let count = pipe.read(&mut buffer)?;
        if count == 0 {
            return Ok(total);
        }
        total = total.saturating_add(count);
    }
}

/// Executes one selected coordinator with fixed repository context and no shell/editor/stage-all/retry.
pub fn run_operation(
    snapshot: &OperationSnapshot,
    action: &OperationAction,
) -> Result<OperationWrite, CoreError> {
    let route = operation_route(snapshot, action)?;
    if prepare_operation(&snapshot.repository.root)? != *snapshot {
        return Err(CoreError::Stale);
    }
    let repo = open(&snapshot.repository.root)?;
    let is_start = matches!(
        action,
        OperationAction::Switch(_) | OperationAction::Merge(_) | OperationAction::Rebase(_)
    );
    if is_start && snapshot.state.is_some() {
        return Err(CoreError::ActiveOperation);
    }
    let unresolved = || -> Result<bool, CoreError> {
        Ok(repo
            .index_or_empty()
            .map_err(git_error)?
            .entries()
            .iter()
            .any(|entry| entry.stage() != gix::index::entry::Stage::Unconflicted))
    };
    if (is_start || *action == OperationAction::Continue) && unresolved()? {
        return Err(CoreError::Unresolved);
    }
    let mut args: Vec<String> = Vec::new();
    match action {
        OperationAction::Switch(branch) => {
            target(&repo, branch)?;
            args.extend([
                "switch".into(),
                "--no-guess".into(),
                "--no-recurse-submodules".into(),
                branch.name.clone(),
            ]);
        }
        OperationAction::Merge(branch) => {
            target(&repo, branch)?;
            if snapshot.repository.head.is_none() {
                return Err(CoreError::Unborn);
            }
            args.extend([
                "merge".into(),
                "--no-edit".into(),
                "--no-autostash".into(),
                "--commit".into(),
                "--no-squash".into(),
                "--ff".into(),
                "-m".into(),
                format!("Merge branch '{}'", branch.name),
                branch.id.clone(),
            ]);
        }
        OperationAction::Rebase(branch) => {
            target(&repo, branch)?;
            if snapshot.repository.head.is_none() {
                return Err(CoreError::Unborn);
            }
            args.extend([
                "rebase".into(),
                "--merge".into(),
                "--no-autostash".into(),
                "--no-autosquash".into(),
                "--no-update-refs".into(),
                "--no-rebase-merges".into(),
                branch.id.clone(),
            ]);
        }
        OperationAction::Continue | OperationAction::Abort | OperationAction::Skip => {
            args.push(
                match snapshot.state.as_ref().ok_or(CoreError::NoOperation)?.kind {
                    Operation::Merge => "merge",
                    Operation::Rebase => "rebase",
                    _ => return Err(CoreError::OperationAction),
                }
                .into(),
            );
            args.push(
                match action {
                    OperationAction::Continue => "--continue",
                    OperationAction::Abort => "--abort",
                    OperationAction::Skip => "--skip",
                    _ => unreachable!(),
                }
                .into(),
            );
        }
    }
    let mut context = repo.command_context().map_err(git_error)?;
    // Paths become explicit after discovery, including linked worktrees and their common object directory.
    context.git_dir = Some(repo.git_dir().canonicalize()?);
    context.worktree_dir = Some(snapshot.repository.root.clone());
    context.stderr = None;
    let prepared = gix::command::prepare("git")
        .with_context(context)
        .env("GIT_INDEX_FILE", repo.index_path().into_os_string())
        .env("GIT_EDITOR", ":")
        .env("GIT_SEQUENCE_EDITOR", ":")
        .env("GIT_MERGE_AUTOEDIT", "no")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["-c", "submodule.recurse=false"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut command: std::process::Command = prepared.into();
    command.current_dir(&snapshot.repository.root);
    for name in [
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_PREFIX",
        "GIT_CONFIG",
    ] {
        command.env_remove(name);
    }
    // No index lock is held across CLI launch: Git acquires and releases its own cooperating locks.
    if prepare_operation(&snapshot.repository.root)? != *snapshot {
        return Err(CoreError::Stale);
    }
    if let OperationAction::Switch(branch)
    | OperationAction::Merge(branch)
    | OperationAction::Rebase(branch) = action
    {
        target(&open(&snapshot.repository.root)?, branch)?;
    }
    let failure = |exit_code| CoreError::OperationExecution {
        capability: route.capability,
        exit_code,
    };
    let mut child = command.spawn().map_err(|_| failure(None))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (status, bytes) = std::thread::scope(|scope| {
        let out = scope.spawn(|| {
            stdout
                .map(drain)
                .transpose()
                .map(|bytes| bytes.unwrap_or(0))
        });
        let err = scope.spawn(|| {
            stderr
                .map(drain)
                .transpose()
                .map(|bytes| bytes.unwrap_or(0))
        });
        let status = child.wait().map_err(|_| failure(None));
        let out = out
            .join()
            .map_err(|_| failure(None))?
            .map_err(|_| failure(None))?;
        let err = err
            .join()
            .map_err(|_| failure(None))?
            .map_err(|_| failure(None))?;
        Ok::<_, CoreError>((status?, out.saturating_add(err)))
    })?;
    let actual = prepare_operation(&snapshot.repository.root)?;
    let conflict = open(&snapshot.repository.root)?
        .index_or_empty()
        .map_err(git_error)?
        .entries()
        .iter()
        .any(|entry| entry.stage() != gix::index::entry::Stage::Unconflicted);
    let can_stop = !matches!(action, OperationAction::Abort | OperationAction::Switch(_));
    let outcome = if status.success() {
        if actual.state.is_some() {
            OperationOutcome::StoppedForReview
        } else {
            OperationOutcome::Completed
        }
    } else if status.code() == Some(1)
        && can_stop
        && conflict
        && (is_start || actual.state != snapshot.state)
        && actual
            .state
            .as_ref()
            .is_some_and(|state| match route.capability {
                OperationCapability::MergeRepositoryCoordinator => state.kind == Operation::Merge,
                OperationCapability::RebaseSequencerCoordinator => state.kind == Operation::Rebase,
                _ => false,
            })
    {
        OperationOutcome::StoppedWithConflicts
    } else {
        return Err(failure(status.code()));
    };
    Ok(OperationWrite {
        route,
        outcome,
        head: actual.repository.head,
        state: actual.state,
        diagnostic_bytes: bytes,
    })
}
