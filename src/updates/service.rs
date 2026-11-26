//! The background updater owned by the primary instance.
//!
//! It makes no request until the user opts in. Enabling runs one check at
//! once, then at most one scheduled check per 24 hours; disabling cancels the
//! check or download in flight and stops further requests.
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use super::apply::{self, Installer, Plan};
use super::store::{Journal, Store, StoreError};
use super::transport::Transport;
use super::updater::{CheckError, Outcome, check_once};
use super::{RootPolicy, Target, Version};
use crate::platform::instance::Primary;

const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// The trusted root policy this build ships with. No owner root has been
/// verified and published yet (#50), so updates are unavailable until a
/// build embeds one.
pub fn embedded_root() -> Option<RootPolicy> {
    // QA builds only (#52): the public test root, with offline checks.
    #[cfg(feature = "qa-offline-update")]
    return Some(super::qa::test_root());
    #[cfg(not(feature = "qa-offline-update"))]
    None
}

/// Why updates cannot run at all, as opposed to one check failing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unavailable {
    /// This build embeds no release signing root.
    NoTrustRoot,
    /// No published artifact exists for this platform.
    UnsupportedPlatform,
    /// No per-user directory is available for the update state.
    NoStateDirectory,
    /// The update state file is damaged and is not reset automatically.
    CorruptState,
    /// Another Token Ashtray process owns the update state.
    LockedElsewhere,
}

/// Why one check failed; the next scheduled check tries again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Failure {
    /// The HTTPS client could not be set up.
    Connection,
    /// The update folders could not be opened or written.
    Storage,
    /// The release channel could not be reached or answered with an error.
    Network,
    /// The release metadata or artifact did not verify.
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateStatus {
    Off,
    Unavailable(Unavailable),
    Checking,
    /// No newer verified release was found.
    UpToDate,
    /// A verified release is downloaded and staged.
    Ready(String),
    Failed(Failure),
    /// The installer started after the last Quit installed this version.
    Installed(String),
    /// The installer started after the last Quit did not install this
    /// version; it is started again after the next Quit.
    InstallFailed {
        version: String,
        attempt: u32,
    },
    /// The installer failed [`apply::MAX_ATTEMPTS`] times for this version, so it
    /// is no longer started automatically.
    InstallStopped(String),
}

/// A check result, told with what the installer started after the last Quit
/// did: an up-to-date app that it updated, or the staged release it failed
/// to install. The journal is settled at the next Quit.
/// `last` is the journal with its failed starts.
fn explain(status: UpdateStatus, last: Option<(Journal, u32)>, installed: Version) -> UpdateStatus {
    let Some((journal, failed)) = last else {
        return status;
    };
    match status {
        UpdateStatus::UpToDate if journal.version == installed.to_string() => {
            UpdateStatus::Installed(journal.version)
        }
        // Starts that another running setup refused are not failures.
        UpdateStatus::Ready(ready) if ready == journal.version && failed > 0 => {
            if apply::exhausted(failed) {
                UpdateStatus::InstallStopped(ready)
            } else {
                UpdateStatus::InstallFailed {
                    version: ready,
                    attempt: failed,
                }
            }
        }
        status => status,
    }
}

/// What a check uses. Its fields are private, so the trust root always comes
/// from [`embedded_root`] outside this module's tests.
pub struct UpdaterConfig {
    dirs: Option<(PathBuf, PathBuf)>,
    bootstrap: Option<RootPolicy>,
    target: Option<Target>,
    installed: Version,
    /// Built on the updater thread before the first check, unless supplied.
    transport: Option<Transport>,
}

/// The inputs of a check that can run.
struct Ready<'a> {
    state: &'a Path,
    staging: &'a Path,
    bootstrap: &'a RootPolicy,
    target: Target,
}

impl UpdaterConfig {
    pub fn production() -> Self {
        Self {
            dirs: Store::default_dirs(),
            bootstrap: embedded_root(),
            target: Target::current(),
            installed: Version::parse(env!("CARGO_PKG_VERSION"))
                .expect("the package version is a stable version"),
            transport: None,
        }
    }

    /// A configuration that never makes a request, whatever the setting.
    pub fn inert() -> Self {
        Self {
            bootstrap: None,
            ..Self::production()
        }
    }

    /// The check inputs, or why updates cannot run, before any request.
    fn ready(&self) -> Result<Ready<'_>, Unavailable> {
        let bootstrap = self.bootstrap.as_ref().ok_or(Unavailable::NoTrustRoot)?;
        let target = self.target.ok_or(Unavailable::UnsupportedPlatform)?;
        let (state, staging) = self.dirs.as_ref().ok_or(Unavailable::NoStateDirectory)?;
        Ok(Ready {
            state,
            staging,
            bootstrap,
            target,
        })
    }

    async fn check(&mut self) -> UpdateStatus {
        if let Err(reason) = self.ready() {
            return UpdateStatus::Unavailable(reason);
        }
        // A QA build never makes a request, so it needs no HTTPS client.
        if cfg!(not(feature = "qa-offline-update")) && self.transport.is_none() {
            self.transport = Transport::new().ok();
        }
        let ready = match self.ready() {
            Ok(ready) => ready,
            Err(reason) => return UpdateStatus::Unavailable(reason),
        };
        let store = match Store::open(ready.state, ready.staging) {
            Ok(Some(store)) => store,
            Ok(None) => return UpdateStatus::Unavailable(Unavailable::LockedElsewhere),
            Err(_) => return UpdateStatus::Failed(Failure::Storage),
        };
        let status = self.check_store(&store, &ready).await;
        let last = store.journal().map(|journal| {
            let failed = store.failed_starts(&journal);
            (journal, failed)
        });
        explain(status, last, self.installed)
    }

    async fn check_store(&self, store: &Store, ready: &Ready<'_>) -> UpdateStatus {
        // QA builds only (#52): what the QA seed staged, without a request.
        #[cfg(feature = "qa-offline-update")]
        return super::qa::offline_check(store, ready.target, self.installed);
        #[cfg(not(feature = "qa-offline-update"))]
        self.download(store, ready).await
    }

    /// Checks the channel and downloads a newer verified release.
    #[cfg_attr(feature = "qa-offline-update", allow(dead_code))]
    async fn download(&self, store: &Store, ready: &Ready<'_>) -> UpdateStatus {
        let Some(transport) = &self.transport else {
            return UpdateStatus::Failed(Failure::Connection);
        };
        let outcome = check_once(
            store,
            transport,
            ready.bootstrap,
            ready.target,
            self.installed,
        )
        .await;
        match outcome {
            Ok(Outcome::NoUpdate) => UpdateStatus::UpToDate,
            Ok(Outcome::Staged { record, .. }) => UpdateStatus::Ready(record.version.to_string()),
            Err(CheckError::Store(StoreError::Corrupt)) => {
                UpdateStatus::Unavailable(Unavailable::CorruptState)
            }
            Err(CheckError::Store(_)) => UpdateStatus::Failed(Failure::Storage),
            Err(CheckError::Fetch(_)) => UpdateStatus::Failed(Failure::Network),
            Err(CheckError::Rejected(_)) => UpdateStatus::Failed(Failure::Rejected),
        }
    }
}

/// What the step after Quit needs from a configuration that can run.
struct QuitInputs {
    state: PathBuf,
    staging: PathBuf,
    target: Target,
}

/// Runs checks on its own thread while enabled. Dropping it stops the thread
/// and cancels any check in flight.
pub struct Updater {
    enabled: watch::Sender<bool>,
    status: Arc<RwLock<UpdateStatus>>,
    thread: std::thread::JoinHandle<()>,
    /// `None` when updates cannot run.
    quit: Option<QuitInputs>,
    installed: Version,
}

impl Updater {
    /// Requires the instance guard, so a UI child, which never holds one,
    /// cannot own an updater. When the guard is unavailable and the app runs
    /// unguarded, the store lock still lets only one process check at a time.
    /// `notify` runs after every status change.
    pub fn start(
        _owner: &Primary,
        config: UpdaterConfig,
        enabled: bool,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let (sender, receiver) = watch::channel(enabled);
        let status = Arc::new(RwLock::new(UpdateStatus::Off));
        let quit = config.ready().ok().map(|ready| QuitInputs {
            state: ready.state.to_owned(),
            staging: ready.staging.to_owned(),
            target: ready.target,
        });
        let installed = config.installed;
        let shared = Arc::clone(&status);
        let thread = std::thread::Builder::new()
            .name("token-ashtray-updater".into())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                let publish = |next: UpdateStatus| {
                    *shared.write().expect("update status lock") = next;
                    notify();
                };
                runtime.block_on(run(config, receiver, publish));
                // Abandon blocking DNS work instead of waiting for it.
                runtime.shutdown_background();
            })
            .expect("spawn the updater thread");
        Self {
            enabled: sender,
            status,
            thread,
            quit,
            installed,
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.send_if_modified(|current| {
            let changed = *current != enabled;
            *current = enabled;
            changed
        });
    }

    pub fn status(&self) -> UpdateStatus {
        self.status.read().expect("update status lock").clone()
    }

    /// The step after the app quits. It first stops the updater thread, so a
    /// check in flight is cancelled and releases the store. Then it
    /// re-verifies the pending release offline and hands its staged installer
    /// to `installer` while updates are on, or clears staging when they are
    /// off. `running` is this executable.
    pub fn after_exit(self, installer: &dyn Installer, running: &Path) -> io::Result<Plan> {
        let enabled = *self.enabled.borrow();
        let Self {
            enabled: sender,
            thread,
            quit,
            installed,
            ..
        } = self;
        drop(sender);
        let _ = thread.join();
        let Some(quit) = quit else {
            return Ok(Plan::Skip(apply::Skip::Unavailable));
        };
        if !enabled && !quit.staging.exists() {
            // Never opted in: nothing to clear, so create no update files.
            return Ok(Plan::Skip(apply::Skip::Disabled));
        }
        let Some(store) = Store::open(&quit.state, &quit.staging)? else {
            return Ok(Plan::Skip(apply::Skip::Unavailable));
        };
        apply::after_exit(&store, enabled, quit.target, installed, running, installer)
    }
}

/// Waits for a setting change; false once the [`Updater`] is dropped.
async fn changed(enabled: &mut watch::Receiver<bool>) -> bool {
    enabled.changed().await.is_ok()
}

async fn run(
    mut config: UpdaterConfig,
    mut enabled: watch::Receiver<bool>,
    publish: impl Fn(UpdateStatus),
) {
    loop {
        if !*enabled.borrow_and_update() {
            publish(UpdateStatus::Off);
            if !changed(&mut enabled).await {
                return;
            }
            continue;
        }
        if config.ready().is_ok() {
            publish(UpdateStatus::Checking);
        }
        // A setting change cancels the check, including its download.
        tokio::select! {
            status = config.check() => publish(status),
            alive = changed(&mut enabled) => {
                if !alive {
                    return;
                }
                continue;
            }
        }
        tokio::select! {
            () = tokio::time::sleep(CHECK_INTERVAL) => {}
            alive = changed(&mut enabled) => {
                if !alive {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
