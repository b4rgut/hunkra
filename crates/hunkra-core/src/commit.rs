//! Index-only commits assembled from official gix tree, signing, command and ref primitives.
use crate::{
    CoreError,
    repository::{git_error, index_digest, open, resolved_head},
};
use gix::{
    bstr::ByteSlice,
    refs::transaction::{PreviousValue, RefEdit},
};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
};

const MAX_MESSAGE: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitSnapshot {
    pub root: PathBuf,
    pub head: Option<String>,
    pub backend: &'static str,
    index: Option<Vec<u8>>,
    head_file: Vec<u8>,
    operation: Option<String>,
}

#[derive(Debug)]
pub struct CommitWrite {
    pub id: String,
    pub backend: &'static str,
    pub signed: bool,
    pub post_hook_failed: bool,
    pub reference_warning: bool,
}

/// Binds a commit request to the precise index, HEAD value, symbolic identity and operation.
fn capture(repo: &gix::Repository) -> Result<CommitSnapshot, CoreError> {
    Ok(CommitSnapshot {
        root: repo.workdir().ok_or(CoreError::Bare)?.canonicalize()?,
        head: resolved_head(repo)?,
        backend: "gix",
        index: index_digest(&repo.index_path())?,
        head_file: fs::read(repo.git_dir().join("HEAD"))?,
        operation: repo.state().map(|state| format!("{state:?}")),
    })
}

/// Obtains a read-only commit precondition without writing objects, index, messages or refs.
pub fn prepare_commit(root: &Path) -> Result<CommitSnapshot, CoreError> {
    capture(&open(root)?)
}

/// Resolves Git's trusted hooksPath relative to the worktree, including linked-worktree defaults.
fn hooks_directory(repo: &gix::Repository) -> Result<PathBuf, CoreError> {
    let path = repo
        .config_snapshot()
        .trusted_path("core.hooksPath")
        .map_err(git_error)?
        .unwrap_or_else(|| repo.common_dir().join("hooks"));
    Ok(if path.is_absolute() {
        path
    } else {
        repo.workdir().ok_or(CoreError::Bare)?.join(path)
    })
}

/// Mirrors Git's executable-hook check; I/O errors remain errors instead of silently skipping it.
fn executable(path: &Path) -> Result<bool, CoreError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        Ok(metadata.is_file())
    }
}

/// Runs a configured Git hook through gix command/context; owned errors never expose its output.
pub(crate) fn hook(
    repo: &gix::Repository,
    name: &'static str,
    args: &[&Path],
    input: Option<&[u8]>,
) -> Result<(), CoreError> {
    let path = hooks_directory(repo)?.join(name);
    if !executable(&path)? {
        return Ok(());
    }
    let mut context = repo.command_context().map_err(git_error)?;
    context.stderr = Some(false);
    let child = gix::command::prepare(path.into_os_string())
        .with_context(context)
        .env("GIT_EDITOR", ":")
        .env("GIT_INDEX_FILE", repo.index_path().into_os_string())
        .args(args.iter().map(|path| path.as_os_str()))
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut command: std::process::Command = child.into();
    command.current_dir(repo.workdir().ok_or(CoreError::Bare)?);
    let mut child = command.spawn().map_err(|_| CoreError::Hook(name))?;
    if let Some(input) = input
        && let Some(mut pipe) = child.stdin.take()
    {
        // A successful hook may intentionally ignore stdin; its exit status decides acceptance.
        let _ = pipe.write_all(input);
    }
    if !child.wait().map_err(|_| CoreError::Hook(name))?.success() {
        return Err(CoreError::Hook(name));
    }
    Ok(())
}

/// Applies noninteractive Git cleanup semantics and bounds any hook-edited UTF-8 message.
fn clean_message(repo: &gix::Repository, message: &str) -> Result<String, CoreError> {
    if message.len() > MAX_MESSAGE || message.contains('\0') {
        return Err(CoreError::CommitMessage);
    }
    let config = repo.config_snapshot();
    let mode = config
        .string("commit.cleanup")
        .unwrap_or_else(|| "default".into());
    if mode == "verbatim" {
        return if message.trim().is_empty() {
            Err(CoreError::CommitMessage)
        } else {
            Ok(message.to_owned())
        };
    }
    if !matches!(
        mode.as_bytes(),
        b"default" | b"whitespace" | b"strip" | b"scissors"
    ) {
        return Err(CoreError::CommitUnsupported("cleanup"));
    }
    let comment = config
        .string("core.commentString")
        .or_else(|| config.string("core.commentChar"))
        .unwrap_or_else(|| "#".into());
    if mode == "strip" && comment == "auto" {
        return Err(CoreError::CommitUnsupported("comment-auto"));
    }
    let mut result = String::new();
    let mut blank = false;
    for line in message.lines() {
        let line = line.trim_end_matches([' ', '\t', '\r']);
        if mode == "strip" && line.as_bytes().starts_with(comment.as_bytes()) {
            continue;
        }
        if line.is_empty() {
            if !result.is_empty() {
                blank = true;
            }
        } else {
            if blank {
                result.push('\n');
                blank = false;
            }
            result.push_str(line);
            result.push('\n');
        }
    }
    if result.is_empty() {
        return Err(CoreError::CommitMessage);
    }
    Ok(result)
}

/// Builds exactly the staged tree; intent-to-add entries contribute no tracked content.
fn index_tree(
    repo: &gix::Repository,
    index: &gix::index::State,
) -> Result<gix::hash::ObjectId, CoreError> {
    let mut tree = repo.empty_tree().edit().map_err(git_error)?;
    for entry in index.entries() {
        if entry.stage() != gix::index::entry::Stage::Unconflicted {
            return Err(CoreError::Unresolved);
        }
        if entry
            .flags
            .contains(gix::index::entry::Flags::INTENT_TO_ADD)
        {
            continue;
        }
        let mode = entry
            .mode
            .to_tree_entry_mode()
            .ok_or(CoreError::CommitUnsupported("mode"))?;
        tree.upsert(entry.path(index), mode.kind(), entry.id)
            .map_err(git_error)?;
    }
    Ok(tree.write().map_err(git_error)?.detach())
}

/// Creates a commit from the approved index; errors never trigger another backend or unsigned retry.
pub fn create_commit(snapshot: &CommitSnapshot, message: &str) -> Result<CommitWrite, CoreError> {
    let repo = open(&snapshot.root)?;
    if capture(&repo)? != *snapshot {
        return Err(CoreError::Stale);
    }
    if snapshot.operation.is_some() {
        return Err(CoreError::ActiveOperation);
    }
    if matches!(&repo.head().map_err(git_error)?.kind, gix::head::Kind::Symbolic(reference) if reference.target.try_id().is_none())
    {
        return Err(CoreError::CommitUnsupported("symbolic-head"));
    }
    let message = clean_message(&repo, message)?;
    if repo
        .config_snapshot()
        .string("i18n.commitEncoding")
        .is_some_and(|encoding| !encoding.eq_ignore_ascii_case(b"UTF-8"))
    {
        return Err(CoreError::CommitUnsupported("encoding"));
    }
    let author = repo
        .author()
        .ok_or(CoreError::Identity)?
        .map_err(|_| CoreError::Identity)?;
    let committer = repo
        .committer()
        .ok_or(CoreError::Identity)?
        .map_err(|_| CoreError::Identity)?;
    if author.name.is_empty()
        || author.email.is_empty()
        || committer.name.is_empty()
        || committer.email.is_empty()
    {
        return Err(CoreError::Identity);
    }
    // Freeze configured signing for this attempt; a hook/config reload cannot downgrade it.
    repo.config_snapshot()
        .try_boolean("commit.gpgSign")
        .map_err(|_| CoreError::Signing)?;
    let signing = repo
        .commit_signing_options_if_enabled()
        .map_err(|_| CoreError::Signing)?;
    let before = repo.index_or_empty().map_err(git_error)?;
    if before.is_sparse() {
        return Err(CoreError::CommitUnsupported("sparse"));
    }
    if before
        .entries()
        .iter()
        .any(|entry| entry.stage() != gix::index::entry::Stage::Unconflicted)
    {
        return Err(CoreError::Unresolved);
    }
    // Hooks can legitimately use Git themselves, so index.lock is acquired after hook execution.
    hook(&repo, "pre-commit", &[], None)?;
    if capture(&open(&snapshot.root)?)? != *snapshot {
        return Err(CoreError::Stale);
    }
    let index_lock = gix::lock::Marker::acquire_to_hold_resource(
        repo.index_path(),
        gix::lock::acquire::Fail::Immediately,
        None,
    )
    .map_err(|_| CoreError::IndexLock)?;
    let fresh = open(&snapshot.root)?;
    if capture(&fresh)? != *snapshot {
        return Err(CoreError::Stale);
    }
    let message_path = repo.git_dir().join("COMMIT_EDITMSG");
    // Atomic replacement does not follow a pre-existing message symlink outside the repository.
    let mut message_lock = gix::lock::File::acquire_to_update_resource(
        &message_path,
        gix::lock::acquire::Fail::Immediately,
        None,
    )
    .map_err(git_error)?;
    message_lock.write_all(message.as_bytes())?;
    message_lock.flush()?;
    message_lock.with_mut(|file| file.sync_all())?;
    message_lock.commit().map_err(git_error)?;
    hook(
        &repo,
        "prepare-commit-msg",
        &[&message_path, Path::new("message")],
        None,
    )?;
    hook(&repo, "commit-msg", &[&message_path], None)?;
    let mut bytes = Vec::new();
    fs::File::open(&message_path)?
        .take((MAX_MESSAGE + 1) as u64)
        .read_to_end(&mut bytes)?;
    let message = clean_message(
        &repo,
        std::str::from_utf8(&bytes).map_err(|_| CoreError::CommitMessage)?,
    )?;
    if capture(&open(&snapshot.root)?)? != *snapshot {
        return Err(CoreError::Stale);
    }
    let index = fresh.index_or_empty().map_err(git_error)?;
    let tree = index_tree(&fresh, &index)?;
    let parent = fresh.head().map_err(git_error)?.id().map(|id| id.detach());
    let old_tree = parent
        .map(|id| {
            fresh
                .find_commit(id)
                .and_then(|commit| commit.tree_id().map_err(Into::into))
                .map(|id| id.detach())
        })
        .transpose()
        .map_err(git_error)?;
    if old_tree == Some(tree) || (parent.is_none() && tree.is_empty_tree()) {
        return Err(CoreError::NothingStaged);
    }
    let commit = fresh
        .new_commit_as(committer, author, &message, tree, parent)
        .map_err(git_error)?;
    let signed = signing.is_some();
    let id = if let Some(options) = signing {
        let commit = commit
            .decode()
            .map_err(|_| CoreError::Signing)?
            .sign(options)
            .map_err(|_| CoreError::Signing)?;
        fresh
            .write_object(commit)
            .map_err(|_| CoreError::Signing)?
            .detach()
    } else {
        commit.id().detach()
    };
    let expected = parent.map_or(PreviousValue::MustNotExist, |old| {
        PreviousValue::MustExistAndMatch(gix::refs::Target::Object(old))
    });
    let edit = RefEdit::update(
        "HEAD".try_into().map_err(git_error)?,
        id,
        expected,
        gix::reference::log::message(
            "commit",
            message.as_str().into(),
            usize::from(parent.is_some()),
        ),
    )
    .with_deref(true);
    let old = parent.unwrap_or_else(|| fresh.object_hash().null());
    let queued = format!("{old} {id} HEAD\n");
    hook(
        &fresh,
        "reference-transaction",
        &[Path::new("preparing")],
        Some(queued.as_bytes()),
    )?;
    let head = fresh.head().map_err(git_error)?;
    let referent = head
        .referent_name()
        .map_or_else(|| "HEAD".into(), |name| name.as_bstr().to_owned());
    let mut resolved = format!("{old} {id} ").into_bytes();
    resolved.extend(referent.as_bytes());
    resolved.push(b'\n');
    let transaction = fresh
        .refs
        .transaction()
        .prepare(
            [edit],
            gix::lock::acquire::Fail::Immediately,
            gix::lock::acquire::Fail::Immediately,
        )
        .map_err(|_| CoreError::ReferenceUpdate)?;
    // Prepared gix refs hold HEAD and referent locks; this catches same-oid branch switches too.
    if capture(&open(&snapshot.root)?)? != *snapshot {
        drop(transaction);
        let _ = hook(
            &fresh,
            "reference-transaction",
            &[Path::new("aborted")],
            Some(&resolved),
        );
        return Err(CoreError::Stale);
    }
    // Rejection in prepared drops the locked transaction; Git does not emit aborted for that case.
    hook(
        &fresh,
        "reference-transaction",
        &[Path::new("prepared")],
        Some(&resolved),
    )?;
    if capture(&open(&snapshot.root)?)? != *snapshot {
        drop(transaction);
        let _ = hook(
            &fresh,
            "reference-transaction",
            &[Path::new("aborted")],
            Some(&resolved),
        );
        return Err(CoreError::Stale);
    }
    let reference_warning = transaction.commit(Some(committer)).is_err();
    if reference_warning && prepare_commit(&snapshot.root)?.head.as_deref() != Some(&id.to_string())
    {
        return Err(CoreError::ReferenceUpdate);
    }
    drop(index_lock);
    let ref_hook_failed = hook(
        &fresh,
        "reference-transaction",
        &[Path::new("committed")],
        Some(&resolved),
    )
    .is_err();
    let post_hook_failed = hook(&fresh, "post-commit", &[], None).is_err() || ref_hook_failed;
    Ok(CommitWrite {
        id: id.to_string(),
        backend: "gix",
        signed,
        post_hook_failed,
        reference_warning,
    })
}
