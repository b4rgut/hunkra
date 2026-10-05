//! Preselected push/fast-forward coordinators; transport errors never trigger a second executor.
use super::{
    NetworkSnapshot, fetch_remote, named_remote, prepare_network, set_upstream,
    transport_supported, upstream_write_supported,
};
use crate::{CoreError, branches::branch_name, repository::open};
use std::{
    io::Read,
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkCapability {
    PushCoordinator,
    FastForwardWorktreeCoordinator,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetworkRoute {
    pub backend: &'static str,
    pub capability: NetworkCapability,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushRequest {
    pub remote: String,
    pub branch: String,
    pub set_upstream: bool,
}
#[derive(Clone, Debug)]
pub struct NetworkWrite {
    pub route: NetworkRoute,
    pub fetch_backend: Option<&'static str>,
    pub head: Option<String>,
    pub upstream_written: bool,
    pub diagnostic_bytes: usize,
}

/// Refuses changed reviewed state, detached/unborn HEAD, unresolved stages and an active operation before transport.
fn current_branch(snapshot: &NetworkSnapshot) -> Result<&str, CoreError> {
    if prepare_network(&snapshot.repository.repository.root)? != *snapshot {
        return Err(CoreError::Stale);
    }
    if snapshot.repository.state.is_some() {
        return Err(CoreError::ActiveOperation);
    }
    if snapshot.repository.repository.head.is_none() {
        return Err(CoreError::Unborn);
    }
    let branch = snapshot
        .current_branch
        .as_deref()
        .ok_or(CoreError::Upstream)?;
    branch_name(branch)?;
    let repo = open(&snapshot.repository.repository.root)?;
    if repo
        .index_or_empty()
        .map_err(|_| CoreError::NetworkConfig)?
        .entries()
        .iter()
        .any(|entry| entry.stage() != gix::index::entry::Stage::Unconflicted)
    {
        return Err(CoreError::Unresolved);
    }
    Ok(branch)
}

/// Discards untrusted child output with fixed memory while preserving only an aggregate byte count.
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

/// Runs the selected complete coordinator once with linked-worktree context and reaped diagnostic pipes.
fn execute(
    snapshot: &NetworkSnapshot,
    capability: NetworkCapability,
    args: Vec<String>,
    cancel: &AtomicBool,
) -> Result<usize, CoreError> {
    let failure = |exit_code| CoreError::NetworkCoordinator {
        capability,
        exit_code,
    };
    let repo = open(&snapshot.repository.repository.root)?;
    let mut context = repo
        .command_context()
        .map_err(|_| CoreError::NetworkConfig)?;
    context.git_dir = Some(repo.git_dir().canonicalize()?);
    context.worktree_dir = Some(snapshot.repository.repository.root.clone());
    context.stderr = None;
    let prepared = gix::command::prepare("git")
        .with_context(context)
        .env("GIT_INDEX_FILE", repo.index_path().into_os_string())
        .env("GIT_EDITOR", ":")
        .env("GIT_MERGE_AUTOEDIT", "no")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "-c",
            "submodule.recurse=false",
            "-c",
            "gc.auto=0",
            "-c",
            "maintenance.auto=false",
            "-c",
            "pack.threads=2",
            "-c",
            "push.autoSetupRemote=false",
        ])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut command: std::process::Command = prepared.into();
    command.current_dir(&snapshot.repository.repository.root);
    for name in [
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_PREFIX",
        "GIT_CONFIG",
    ] {
        command.env_remove(name);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(CoreError::NetworkCanceled);
    }
    current_branch(snapshot)?;
    // Git owns its own cooperating locks; an accepted child is waited, never killed or retried after a partial write.
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
    if !status.success() {
        return Err(failure(status.code()));
    }
    Ok(bytes)
}

/// Pushes one explicit current local branch without mirror/tags/force and configures upstream only after success.
pub fn push_branch(
    snapshot: &NetworkSnapshot,
    request: &PushRequest,
    cancel: &AtomicBool,
) -> Result<NetworkWrite, CoreError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(CoreError::NetworkCanceled);
    }
    let branch = current_branch(snapshot)?;
    let source = branch_name(branch)?;
    let destination = branch_name(&request.branch)?;
    let repo = open(&snapshot.repository.repository.root)?;
    let remote = named_remote(&repo, snapshot, &request.remote)?;
    if remote.urls(gix::remote::Direction::Push).count() != 1
        || request.remote.contains(['=', '\n', '\r'])
        || !remote
            .url(gix::remote::Direction::Push)
            .is_some_and(transport_supported)
        || repo
            .config_snapshot()
            .raw_values_by("remotes", None, &request.remote)
            .is_ok()
    {
        return Err(CoreError::NetworkConfig);
    }
    if request.set_upstream {
        upstream_write_supported(snapshot, &repo, branch)?;
    }
    let capability = NetworkCapability::PushCoordinator;
    let diagnostic_bytes = execute(
        snapshot,
        capability,
        vec![
            "-c".into(),
            format!("remote.{}.mirror=false", request.remote),
            "push".into(),
            "--no-force".into(),
            "--no-mirror".into(),
            "--no-follow-tags".into(),
            "--no-prune".into(),
            "--recurse-submodules=no".into(),
            "--verify".into(),
            "--".into(),
            request.remote.clone(),
            format!("{source}:{destination}"),
        ],
        cancel,
    )?;
    let actual = prepare_network(&snapshot.repository.repository.root)?;
    // A hook may have changed local data even after a confirmed remote push: do not silently approve that config write.
    if request.set_upstream {
        if actual.repository != snapshot.repository || actual.config != snapshot.config {
            return Err(CoreError::Stale);
        }
        set_upstream(&actual, &request.remote, &request.branch)?;
    }
    Ok(NetworkWrite {
        route: NetworkRoute {
            backend: "git-cli",
            capability,
        },
        fetch_backend: None,
        head: actual.repository.repository.head,
        upstream_written: request.set_upstream,
        diagnostic_bytes,
    })
}

/// Fetches the single reviewed upstream with gix, then integrates its immutable commit by a separate ff-only coordinator.
pub fn pull_ff_only(
    snapshot: &NetworkSnapshot,
    cancel: &AtomicBool,
) -> Result<NetworkWrite, CoreError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(CoreError::NetworkCanceled);
    }
    current_branch(snapshot)?;
    let upstream = snapshot.upstream.as_ref().ok_or(CoreError::Upstream)?;
    let tracking = upstream
        .tracking_ref
        .as_deref()
        .ok_or(CoreError::Upstream)?;
    let fetch = fetch_remote(snapshot, &upstream.remote, cancel)?;
    let fetched = prepare_network(&snapshot.repository.repository.root)?;
    if fetched.repository != snapshot.repository
        || fetched.config != snapshot.config
        || fetched.upstream != snapshot.upstream
    {
        return Err(CoreError::Stale);
    }
    let repo = open(&snapshot.repository.repository.root)?;
    let target = repo
        .find_reference(tracking)
        .map_err(|_| CoreError::Upstream)?
        .peel_to_id()
        .map_err(|_| CoreError::Upstream)?;
    repo.find_commit(target).map_err(|_| CoreError::Upstream)?;
    let capability = NetworkCapability::FastForwardWorktreeCoordinator;
    let diagnostic_bytes = execute(
        &fetched,
        capability,
        vec![
            "merge".into(),
            "--ff-only".into(),
            "--no-autostash".into(),
            "--no-edit".into(),
            "--no-squash".into(),
            "--no-overwrite-ignore".into(),
            target.to_string(),
        ],
        cancel,
    )?;
    let actual = prepare_network(&snapshot.repository.repository.root)?;
    if actual.repository.state.is_some() || actual.current_branch != snapshot.current_branch {
        return Err(CoreError::Stale);
    }
    Ok(NetworkWrite {
        route: NetworkRoute {
            backend: "git-cli",
            capability,
        },
        fetch_backend: Some(fetch.backend),
        head: actual.repository.repository.head,
        upstream_written: false,
        diagnostic_bytes,
    })
}
