//! gix network reads, fresh repository creation and guarded remote-tracking mutations.
mod coordinator;
use crate::{
    CoreError, OperationSnapshot, branches::branch_name, prepare_operation, repository::open,
};
pub use coordinator::{
    NetworkCapability, NetworkRoute, NetworkWrite, PushRequest, pull_ff_only, push_branch,
};
use gix::{bstr::ByteSlice, error::ResultExt};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

const CONFIG_LIMIT: usize = 1024 * 1024;
const REF_LIMIT: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectFormat {
    Sha1,
    Sha256,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteInfo {
    pub name: String,
    pub fetch_supported: bool,
    pub push_supported: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteBranch {
    pub name: String,
    pub id: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Upstream {
    pub remote: String,
    pub branch: String,
    pub tracking_ref: Option<String>,
}
#[derive(Clone, PartialEq, Eq)]
struct ConfigGuard {
    path: PathBuf,
    physical: Vec<u8>,
    effective: Vec<u8>,
}
impl std::fmt::Debug for ConfigGuard {
    /// Never prints credential URLs, headers or helper configuration captured by a private guard.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigGuard").finish_non_exhaustive()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkSnapshot {
    pub repository: OperationSnapshot,
    pub remotes: Vec<RemoteInfo>,
    pub branches: Vec<RemoteBranch>,
    pub current_branch: Option<String>,
    pub upstream: Option<Upstream>,
    config: ConfigGuard,
}
#[derive(Clone, Debug)]
pub struct RepositoryCreated {
    pub root: PathBuf,
    pub backend: &'static str,
}
#[derive(Clone, Debug)]
pub struct FetchWrite {
    pub backend: &'static str,
    pub updated_refs: usize,
}

struct LimitedBytes(Vec<u8>);
impl Write for LimitedBytes {
    /// Bounds effective configuration serialization before allocating or retaining excess bytes.
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > CONFIG_LIMIT.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("configuration limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    /// Configuration serialization does not need a separate in-memory flush.
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Captures one regular local config and effective includes without exposing their potentially secret bytes.
fn config_guard(repo: &gix::Repository) -> Result<ConfigGuard, CoreError> {
    let path = repo
        .config_path(gix::config::Source::Local)
        .map_err(|_| CoreError::NetworkConfig)?;
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_file() || metadata.len() > CONFIG_LIMIT as u64 {
        return Err(CoreError::NetworkConfig);
    }
    let mut physical = Vec::new();
    fs::File::open(&path)?
        .take(CONFIG_LIMIT as u64 + 1)
        .read_to_end(&mut physical)?;
    if physical.len() > CONFIG_LIMIT {
        return Err(CoreError::NetworkConfig);
    }
    let mut effective = LimitedBytes(Vec::new());
    repo.config_snapshot()
        .write_to(&mut effective)
        .map_err(|_| CoreError::NetworkConfig)?;
    Ok(ConfigGuard {
        path,
        physical,
        effective: effective.0,
    })
}

/// Accepts built-in transports only; application routes never silently invoke an unaudited remote helper.
fn transport_supported(url: &gix::Url) -> bool {
    matches!(
        url.scheme,
        gix::url::Scheme::File
            | gix::url::Scheme::Git
            | gix::url::Scheme::Ssh
            | gix::url::Scheme::Http
            | gix::url::Scheme::Https
    )
}
/// Requires a real named remote, avoiding gix's intentional URL-shaped name fallback.
fn named_remote<'a>(
    repo: &'a gix::Repository,
    snapshot: &NetworkSnapshot,
    name: &str,
) -> Result<gix::Remote<'a>, CoreError> {
    if name.starts_with('-') || !snapshot.remotes.iter().any(|remote| remote.name == name) {
        return Err(CoreError::NetworkTarget);
    }
    repo.find_remote(name).map_err(|_| CoreError::NetworkConfig)
}

/// Enumerates only owned remote names/tracking refs/upstream; endpoint credentials remain private.
pub fn prepare_network(root: &Path) -> Result<NetworkSnapshot, CoreError> {
    let repository = prepare_operation(root)?;
    let repo = open(&repository.repository.root)?;
    let config = config_guard(&repo)?;
    let mut remotes = Vec::new();
    for name in repo.remote_names() {
        if remotes.len() >= 1000 {
            return Err(CoreError::NetworkConfig);
        }
        let name = std::str::from_utf8(&name)
            .map_err(|_| CoreError::NetworkConfig)?
            .to_owned();
        if name.is_empty() || name.starts_with('-') || name.len() > 4096 {
            return Err(CoreError::NetworkConfig);
        }
        let remote = repo
            .find_remote(name.as_str())
            .map_err(|_| CoreError::NetworkConfig)?;
        remotes.push(RemoteInfo {
            name,
            fetch_supported: remote
                .url(gix::remote::Direction::Fetch)
                .is_some_and(transport_supported),
            push_supported: remote.urls(gix::remote::Direction::Push).count() == 1
                && remote
                    .url(gix::remote::Direction::Push)
                    .is_some_and(transport_supported),
        });
    }
    let names = {
        let references = repo.references().map_err(|_| CoreError::NetworkConfig)?;
        let mut names = Vec::new();
        for reference in references
            .remote_branches()
            .map_err(|_| CoreError::NetworkConfig)?
        {
            if names.len() >= REF_LIMIT {
                return Err(CoreError::ReferenceLimit);
            }
            let reference = reference.map_err(|_| CoreError::NetworkConfig)?;
            names.push(
                std::str::from_utf8(reference.name().as_bstr())
                    .map_err(|_| CoreError::ReferenceLimit)?
                    .to_owned(),
            );
        }
        names
    };
    let mut branches = Vec::new();
    for name in names {
        let reference = repo
            .find_reference(name.as_str())
            .map_err(|_| CoreError::NetworkConfig)?;
        let id = match reference.clone().peel_to_id() {
            Ok(id) => id.to_string(),
            Err(error) if reference.try_id().is_none() && error.is_not_found() => continue,
            Err(_) => return Err(CoreError::NetworkConfig),
        };
        branches.push(RemoteBranch {
            name: name
                .strip_prefix("refs/remotes/")
                .ok_or(CoreError::NetworkConfig)?
                .to_owned(),
            id,
        });
    }
    let head = repo.head().map_err(|_| CoreError::NetworkConfig)?;
    let current_branch = head
        .referent_name()
        .and_then(|name| name.as_bstr().strip_prefix(b"refs/heads/"))
        .map(|name| std::str::from_utf8(name).map(str::to_owned))
        .transpose()
        .map_err(|_| CoreError::NetworkConfig)?;
    let mut upstream = None;
    if let Some(branch) = &current_branch {
        let snapshot = repo.config_snapshot();
        let remote = snapshot
            .raw_value_by("branch", Some(branch.as_bytes().as_bstr()), "remote")
            .ok();
        let merges = snapshot
            .raw_values_by("branch", Some(branch.as_bytes().as_bstr()), "merge")
            .ok()
            .unwrap_or_default();
        if let (Some(remote), [merge]) = (remote, merges.as_slice()) {
            let remote = std::str::from_utf8(&remote)
                .map_err(|_| CoreError::NetworkConfig)?
                .to_owned();
            let branch_name = std::str::from_utf8(merge)
                .map_err(|_| CoreError::NetworkConfig)?
                .to_owned();
            if remotes.iter().any(|entry| entry.name == remote)
                && branch_name.starts_with("refs/heads/")
            {
                let full: gix::refs::FullName = format!("refs/heads/{branch}")
                    .try_into()
                    .map_err(|_| CoreError::NetworkConfig)?;
                let tracking_ref = repo
                    .branch_remote_tracking_ref_name(full.as_ref(), gix::remote::Direction::Fetch)
                    .transpose()
                    .map_err(|_| CoreError::NetworkConfig)?
                    .map(|name| name.as_bstr().to_string());
                upstream = Some(Upstream {
                    remote,
                    branch: branch_name,
                    tracking_ref,
                });
            }
        }
    }
    if prepare_operation(&repository.repository.root)? != repository
        || config_guard(&open(&repository.repository.root)?)? != config
    {
        return Err(CoreError::Stale);
    }
    Ok(NetworkSnapshot {
        repository,
        remotes,
        branches,
        current_branch,
        upstream,
        config,
    })
}

/// Applies conservative thread limits and keeps configured helpers/askpass available without terminal prompting.
fn options(branch: Option<&str>) -> gix::open::Options {
    let mut permission = gix::open::Permissions::default();
    permission.config.git_binary = true;
    let mut overrides = vec![
        "checkout.workers=2".to_owned(),
        "pack.threads=2".to_owned(),
        "core.configLockTimeout=0".to_owned(),
        "gitoxide.core.externalCommandStderr=false".to_owned(),
    ];
    if let Some(branch) = branch {
        overrides.push(format!("init.defaultBranch={branch}"));
    }
    gix::open::Options::default()
        .permissions(permission)
        .config_overrides(overrides)
}
/// Initializes a new .git with an explicit branch/hash, preserving all existing working files.
pub fn init_repository(
    root: &Path,
    branch: &str,
    format: ObjectFormat,
) -> Result<RepositoryCreated, CoreError> {
    branch_name(branch)?;
    if !root.is_absolute() {
        return Err(CoreError::RepositoryDestination);
    }
    if fs::symlink_metadata(root.join(".git")).is_ok()
        || fs::symlink_metadata(root).is_ok_and(|meta| !meta.is_dir())
    {
        return Err(CoreError::RepositoryDestination);
    }
    if !root.exists() {
        fs::create_dir(root)?;
    }
    let repo = gix::ThreadSafeRepository::init_opts(
        root,
        gix::create::Kind::WithWorktree,
        gix::create::Options {
            destination_must_be_empty: Some(false),
            object_hash: Some(match format {
                ObjectFormat::Sha1 => gix::hash::Kind::Sha1,
                ObjectFormat::Sha256 => gix::hash::Kind::Sha256,
            }),
            ..Default::default()
        },
        options(Some(branch)),
    )
    .map_err(|_| CoreError::RepositoryDestination)?
    .to_thread_local();
    Ok(RepositoryCreated {
        root: repo.workdir().ok_or(CoreError::Bare)?.canonicalize()?,
        backend: "gix",
    })
}

/// Obtains helpers for the transport's current URL, suppressing helper diagnostics and terminal input.
fn authentication(
    repo: gix::Repository,
) -> Box<dyn FnMut(gix::credentials::helper::Action) -> gix::credentials::protocol::Result> {
    let mut configured: Option<(gix::credentials::helper::Cascade, gix::prompt::Options)> = None;
    Box::new(move |action| {
        if matches!(&action, gix::credentials::helper::Action::Get(_)) {
            let url = action
                .context()
                .and_then(|ctx| ctx.url.clone().or_else(|| ctx.to_url()))
                .ok_or_else(|| std::io::Error::other("credential URL missing"))
                .or_erased()?;
            let (mut cascade, _, mut prompt) = repo
                .config_snapshot()
                .credential_helpers(gix::url::parse(url).or_erased()?)
                .or_erased()?;
            cascade.stderr = false;
            prompt.mode = gix::prompt::Mode::Disable;
            configured = Some((cascade, prompt));
        }
        match configured.as_mut() {
            Some((cascade, prompt)) => cascade.invoke(action, prompt.clone()),
            None => Ok(None),
        }
    })
}

/// Replaces only the not-yet-started SSH transport, preserving its configured command/variant while suppressing arbitrary stderr.
fn quiet_ssh(
    connection: &mut gix::remote::Connection<
        '_,
        '_,
        '_,
        Box<dyn gix::protocol::transport::client::blocking_io::Transport + Send>,
    >,
) -> Result<(), CoreError> {
    use gix::protocol::transport::client::blocking_io::ssh;
    let remote = connection.remote();
    let (url, version) = remote
        .sanitized_url_and_version(gix::remote::Direction::Fetch)
        .map_err(|_| CoreError::NetworkConfig)?;
    if url.scheme != gix::url::Scheme::Ssh {
        return Ok(());
    }
    let mut options = remote
        .repo()
        .ssh_connect_options()
        .map_err(|_| CoreError::NetworkConfig)?;
    let program = options
        .ssh_command()
        .to_str()
        .ok_or(CoreError::NetworkConfig)?;
    let guessed = ssh::ProgramKind::from(options.ssh_command());
    let mut command = if options.disallow_shell {
        String::from_utf8(gix::quote::single(program.as_bytes().as_bstr()).to_vec())
            .map_err(|_| CoreError::NetworkConfig)?
    } else {
        program.to_owned()
    };
    // This is the already configured trusted command; URL/host/path arguments remain separate quoted positional parameters.
    if !command.contains("$@") {
        command.push_str(" \"$@\"");
    }
    options.command = Some(format!("{{ {command}; }} 2>/dev/null").into());
    options.disallow_shell = false;
    if options.kind.is_none() && guessed != ssh::ProgramKind::Simple {
        options.kind = Some(guessed);
    }
    let transport =
        ssh::connect(url, version, options, false).map_err(|_| CoreError::NetworkExecution)?;
    *connection.transport_mut() = Box::new(transport);
    Ok(())
}

/// Checks actual fetched-tree attributes before using the known UTF-16-limited checkout encoder.
fn checkout_supported(repo: &gix::Repository) -> Result<(), CoreError> {
    if repo
        .head()
        .map_err(|_| CoreError::CloneCheckout)?
        .is_unborn()
    {
        return Ok(());
    }
    let tree = repo
        .head_commit()
        .map_err(|_| CoreError::CloneCheckout)?
        .tree_id()
        .map_err(|_| CoreError::CloneCheckout)?
        .detach();
    let index = repo
        .index_from_tree(&tree)
        .map_err(|_| CoreError::CloneCheckout)?;
    let mut attributes = repo
        .attributes_only(
            &index,
            gix::worktree::stack::state::attributes::Source::IdMapping,
        )
        .map_err(|_| CoreError::CloneCheckout)?;
    let mut matches = attributes.selected_attribute_matches(["working-tree-encoding"]);
    for entry in index.entries() {
        if !matches!(entry.mode.bits(), 0o100644 | 0o100755) {
            continue;
        }
        attributes
            .at_entry(entry.path(&index), Some(entry.mode))
            .map_err(|_| CoreError::CloneCheckout)?
            .matching_attributes(&mut matches);
        if matches.iter_selected().any(|attribute| {
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
        }) {
            return Err(CoreError::CloneCheckout);
        }
    }
    Ok(())
}

/// Clones once into an atomically reserved fresh directory and preserves partial data on every failure.
pub fn clone_repository(
    url: &str,
    destination: &Path,
    cancel: &AtomicBool,
) -> Result<RepositoryCreated, CoreError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(CoreError::NetworkCanceled);
    }
    let parsed = gix::url::parse(url).map_err(|_| CoreError::NetworkTarget)?;
    if !transport_supported(&parsed) || url.len() > 16 * 1024 || !destination.is_absolute() {
        return Err(CoreError::NetworkTarget);
    }
    fs::create_dir(destination).map_err(|_| CoreError::RepositoryDestination)?;
    let mut fetch = gix::clone::PrepareFetch::new(
        url,
        destination,
        gix::create::Kind::WithWorktree,
        gix::create::Options {
            destination_must_be_empty: Some(true),
            ..Default::default()
        },
        options(None),
    )
    .map_err(|_| CoreError::NetworkExecution)?
    .with_in_memory_config_overrides(["checkout.workers=2", "pack.threads=2"])
    .configure_remote(|remote| {
        if !remote
            .url(gix::remote::Direction::Fetch)
            .is_some_and(transport_supported)
            || crate::commit::hook_exists(remote.repo(), "reference-transaction").or_erased()?
        {
            return Err(std::io::Error::other(
                "unsupported clone transport or reference hook",
            ))
            .or_erased();
        }
        Ok(remote)
    })
    .configure_connection(|connection| {
        quiet_ssh(connection).or_erased()?;
        connection.set_credentials(authentication(connection.remote().repo().clone()));
        Ok(())
    });
    let fetched = fetch.fetch_then_checkout(gix::progress::Discard, cancel);
    let (mut checkout, _) = match fetched {
        Ok(result) => result,
        Err(_) => {
            fetch.persist();
            return Err(if cancel.load(Ordering::Relaxed) {
                CoreError::NetworkCanceled
            } else {
                CoreError::NetworkExecution
            });
        }
    };
    if let Err(error) = checkout_supported(checkout.repo()) {
        checkout.persist();
        return Err(error);
    }
    match checkout.main_worktree(gix::progress::Discard, cancel) {
        Ok((repo, outcome)) => {
            if cancel.load(Ordering::Relaxed) {
                return Err(CoreError::NetworkCanceled);
            }
            if !outcome.errors.is_empty() || !outcome.collisions.is_empty() {
                return Err(CoreError::CloneCheckout);
            }
            if let Some(head) = repo
                .head()
                .map_err(|_| CoreError::CloneCheckout)?
                .try_peel_to_id()
                .map_err(|_| CoreError::CloneCheckout)?
            {
                let old = repo.object_hash().null().to_string();
                let new = head.to_string();
                crate::commit::hook(
                    &repo,
                    "post-checkout",
                    &[Path::new(&old), Path::new(&new), Path::new("1")],
                    None,
                )?;
            }
            Ok(RepositoryCreated {
                root: repo.workdir().ok_or(CoreError::Bare)?.canonicalize()?,
                backend: "gix",
            })
        }
        Err(_) => {
            checkout.persist();
            Err(if cancel.load(Ordering::Relaxed) {
                CoreError::NetworkCanceled
            } else {
                CoreError::CloneCheckout
            })
        }
    }
}

/// Fetches only configured remote-tracking refs through gix, never local branches or worktree/index.
pub fn fetch_remote(
    snapshot: &NetworkSnapshot,
    name: &str,
    cancel: &AtomicBool,
) -> Result<FetchWrite, CoreError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(CoreError::NetworkCanceled);
    }
    if prepare_network(&snapshot.repository.repository.root)? != *snapshot {
        return Err(CoreError::Stale);
    }
    let mut repo = open(&snapshot.repository.repository.root)?;
    if crate::commit::hook_exists(&repo, "reference-transaction")? {
        return Err(CoreError::NetworkConfig);
    }
    repo.config_snapshot_mut()
        .append_config(["pack.threads=2"], gix::config::Source::Api)
        .map_err(|_| CoreError::NetworkConfig)?;
    repo.committer_or_set_generic_fallback()
        .map_err(|_| CoreError::NetworkConfig)?;
    let remote =
        named_remote(&repo, snapshot, name)?.with_fetch_tags(gix::remote::fetch::Tags::None);
    if !remote
        .url(gix::remote::Direction::Fetch)
        .is_some_and(transport_supported)
        || remote.refspecs(gix::remote::Direction::Fetch).is_empty()
        || remote
            .refspecs(gix::remote::Direction::Fetch)
            .iter()
            .any(|spec| {
                !spec.to_ref().destination().is_some_and(|destination| {
                    destination.starts_with_str(format!("refs/remotes/{name}/"))
                })
            })
    {
        return Err(CoreError::NetworkConfig);
    }
    let mut connection = remote
        .connect(gix::remote::Direction::Fetch)
        .map_err(|_| CoreError::NetworkExecution)?
        .with_credentials(authentication(repo.clone()));
    quiet_ssh(&mut connection)?;
    let prepared = connection
        .prepare_fetch(gix::progress::Discard, Default::default())
        .map_err(|_| CoreError::NetworkExecution)?;
    if prepare_network(&snapshot.repository.repository.root)? != *snapshot {
        return Err(CoreError::Stale);
    }
    let outcome = prepared
        .receive(gix::progress::Discard, cancel)
        .map_err(|_| {
            if cancel.load(Ordering::Relaxed) {
                CoreError::NetworkCanceled
            } else {
                CoreError::NetworkExecution
            }
        })?;
    let refs = match &outcome.status {
        gix::remote::fetch::Status::Change { update_refs, .. }
        | gix::remote::fetch::Status::NoPackReceived { update_refs, .. } => update_refs,
    };
    use gix::remote::fetch::refs::update::Mode;
    if refs.updates.iter().any(|update| {
        matches!(
            update.mode,
            Mode::RejectedSourceObjectNotFound { .. }
                | Mode::RejectedTagUpdate
                | Mode::RejectedNonFastForward
                | Mode::RejectedToReplaceWithUnborn
                | Mode::RejectedCurrentlyCheckedOut { .. }
        )
    }) {
        return Err(CoreError::FetchRejected);
    }
    Ok(FetchWrite {
        backend: "gix",
        updated_refs: refs
            .updates
            .iter()
            .filter(|update| matches!(update.mode, Mode::New | Mode::FastForward | Mode::Forced))
            .count(),
    })
}

/// Refuses upstream values supplied by includes/global config, which a local-only edit cannot safely replace.
fn upstream_write_supported(
    snapshot: &NetworkSnapshot,
    repo: &gix::Repository,
    current: &str,
) -> Result<(), CoreError> {
    let physical = gix::config::File::try_from(snapshot.config.physical.as_bstr())
        .map_err(|_| CoreError::NetworkConfig)?;
    let effective = repo.config_snapshot();
    for key in ["remote", "merge"] {
        let local = physical
            .raw_values_by("branch", Some(current.as_bytes().as_bstr()), key)
            .unwrap_or_default();
        let all = effective
            .raw_values_by("branch", Some(current.as_bytes().as_bstr()), key)
            .unwrap_or_default();
        // merge is multivalued, and a later include can override a scalar remote.
        if all != local || (key == "merge" && all.len() > 1) {
            return Err(CoreError::NetworkConfig);
        }
    }
    Ok(())
}

/// Writes only the current branch's explicit local upstream, under a physical config transaction.
pub fn set_upstream(
    snapshot: &NetworkSnapshot,
    remote: &str,
    branch: &str,
) -> Result<(), CoreError> {
    let full = branch_name(branch)?;
    let current = snapshot
        .current_branch
        .as_deref()
        .ok_or(CoreError::Upstream)?;
    if snapshot.repository.state.is_some() {
        return Err(CoreError::ActiveOperation);
    }
    if prepare_network(&snapshot.repository.repository.root)? != *snapshot {
        return Err(CoreError::Stale);
    }
    let repo = open(&snapshot.repository.repository.root)?;
    named_remote(&repo, snapshot, remote)?;
    upstream_write_supported(snapshot, &repo, current)?;
    let mut config = repo
        .config_file_mut(&snapshot.config.path)
        .map_err(|_| CoreError::ConfigLock)?;
    if prepare_network(&snapshot.repository.repository.root)? != *snapshot {
        return Err(CoreError::Stale);
    }
    config
        .set_raw_value_by(
            "branch",
            Some(current.as_bytes().as_bstr()),
            "remote",
            remote,
        )
        .map_err(|_| CoreError::NetworkConfig)?;
    config
        .set_raw_value_by(
            "branch",
            Some(current.as_bytes().as_bstr()),
            "merge",
            full.as_str(),
        )
        .map_err(|_| CoreError::NetworkConfig)?;
    config.commit().map_err(|_| CoreError::NetworkExecution)
}
