//! One complete update check: discover, verify, persist floors, stage.
use std::path::PathBuf;

use super::store::{Store, StoreError};
use super::transport::{FetchError, Transport};
use super::{ArtifactRecord, Reject, RootPolicy, State, Status, Target, Version, verify};

#[derive(Debug)]
pub enum CheckError {
    Store(StoreError),
    Fetch(FetchError),
    Rejected(Reject),
}

impl From<StoreError> for CheckError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<std::io::Error> for CheckError {
    fn from(error: std::io::Error) -> Self {
        Self::Store(StoreError::Io(error))
    }
}

impl From<FetchError> for CheckError {
    fn from(error: FetchError) -> Self {
        Self::Fetch(error)
    }
}

impl From<Reject> for CheckError {
    fn from(reject: Reject) -> Self {
        Self::Rejected(reject)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// No newer verified release was found.
    NoUpdate,
    /// A verified artifact is staged at `path`. Re-check it with
    /// [`Store::verified`] before handing it over.
    Staged {
        path: PathBuf,
        record: ArtifactRecord,
    },
}

/// Runs one check while holding `store`'s lock. Concurrent calls on the same
/// store run one after another. `bootstrap` is the embedded root policy, used
/// until a state file exists.
pub async fn check_once(
    store: &Store,
    transport: &Transport,
    bootstrap: &RootPolicy,
    target: Target,
    installed: Version,
) -> Result<Outcome, CheckError> {
    let _checking = store.checking.lock().await;
    let mut state = store
        .load()?
        .unwrap_or_else(|| State::initial(bootstrap.clone()));
    let stored = state.clone();
    // The installed binary is trusted as a whole, so a build that embeds a
    // newer root (for example after an offline rotation) supersedes an older
    // stored root. An older or equal embedded root never replaces the store.
    if bootstrap.version() > state.root.version() {
        state.root = bootstrap.clone();
    }
    let found = transport.discover().await?;
    let verification = verify(&found.raw, &state, target, installed, found.version);
    // Persist before acting on the result: an authenticated root advance
    // must survive a rejected release, and floors precede any download.
    if verification.state != stored {
        store.save(&verification.state)?;
    }
    let accepted = verification.result?;
    if accepted.status == Status::NoUpdate {
        store.clean_staging(None)?;
        return Ok(Outcome::NoUpdate);
    }
    // Keep the bundle for an offline re-check at Quit.
    store.save_bundle(&found.raw)?;
    let record = accepted.artifact;
    store.clean_staging(Some(&record.filename))?;
    let path = match store.verified(&record) {
        Some(path) => path,
        None => {
            let staging = store.stage(&record);
            transport.download(&record, staging.partial()).await?;
            staging.commit()?
        }
    };
    Ok(Outcome::Staged { path, record })
}

#[cfg(test)]
mod tests;
