//! Owned remote-state replies; the host chooses the bounded source used by the native panel.
use crate::worker::LatestWorker;
use hunkra_core::{CoreError, network::NetworkSnapshot};
use std::{path::PathBuf, sync::Arc};

pub type NetworkWorker = LatestWorker<PathBuf, Result<Arc<NetworkSnapshot>, CoreError>>;

/// Reads standalone gix state on the existing bounded worker; embedded hosts can supply their own source.
pub fn standalone_reader() -> NetworkWorker {
    LatestWorker::new("hunkra-remotes", |root: PathBuf| {
        hunkra_core::network::prepare_network(&root).map(Arc::new)
    })
}
