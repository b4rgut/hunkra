//! Owned requests captured by native controls and executed exclusively by the host's serialized writer.
use hunkra_core::{
    CoreError,
    network::{
        self, FetchWrite, NetworkSnapshot, NetworkWrite, ObjectFormat, PushRequest,
        RepositoryCreated,
    },
};
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

#[derive(Clone)]
pub enum NetworkCommand {
    Init {
        root: PathBuf,
        branch: String,
        format: ObjectFormat,
    },
    Clone {
        destination: PathBuf,
        url: String,
    },
    Fetch(Arc<NetworkSnapshot>, String),
    Push(Arc<NetworkSnapshot>, PushRequest),
    Upstream(Arc<NetworkSnapshot>, String, String),
    Pull(Arc<NetworkSnapshot>),
}
pub enum NetworkOutcome {
    Created(RepositoryCreated),
    Fetch(FetchWrite),
    Upstream,
    Coordinator(NetworkWrite),
}
impl NetworkOutcome {
    /// Keeps current diff coordinates after ref/config-only writes; refresh still verifies their actual document version.
    pub fn changes_documents(&self) -> bool {
        match self {
            Self::Created(_) => true,
            Self::Coordinator(write) => {
                write.route.capability == network::NetworkCapability::FastForwardWorktreeCoordinator
            }
            Self::Fetch(_) | Self::Upstream => false,
        }
    }
}
impl NetworkCommand {
    /// Names commands that may replace the visible project or its working files and therefore need the host draft guard.
    pub fn changes_workspace(&self) -> bool {
        matches!(self, Self::Init { .. } | Self::Clone { .. } | Self::Pull(_))
    }
}
/// Dispatches one frozen owned request; an accepted CLI coordinator is waited and never killed/retried by the host.
pub fn run(command: NetworkCommand) -> Result<NetworkOutcome, CoreError> {
    let cancel = AtomicBool::new(false);
    match command {
        NetworkCommand::Init {
            root,
            branch,
            format,
        } => network::init_repository(&root, &branch, format).map(NetworkOutcome::Created),
        NetworkCommand::Clone { destination, url } => {
            network::clone_repository(&url, &destination, &cancel).map(NetworkOutcome::Created)
        }
        NetworkCommand::Fetch(snapshot, remote) => {
            network::fetch_remote(&snapshot, &remote, &cancel).map(NetworkOutcome::Fetch)
        }
        NetworkCommand::Push(snapshot, request) => {
            network::push_branch(&snapshot, &request, &cancel).map(NetworkOutcome::Coordinator)
        }
        NetworkCommand::Upstream(snapshot, remote, branch) => {
            network::set_upstream(&snapshot, &remote, &branch).map(|_| NetworkOutcome::Upstream)
        }
        NetworkCommand::Pull(snapshot) => {
            network::pull_ff_only(&snapshot, &cancel).map(NetworkOutcome::Coordinator)
        }
    }
}
