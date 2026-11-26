//! Applying a staged update after the app quits, shared by every platform.
//!
//! The pending artifact is re-verified offline from the stored bundle and its
//! bytes are re-hashed, so a Quit in a later session needs no network. A
//! platform [`Installer`] locates the installation this process runs from
//! and starts the verified installer for it. Each start is journaled, and a
//! release whose installer keeps failing stops being retried after
//! [`MAX_ATTEMPTS`].
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use super::store::{Journal, Store};
use super::{ArtifactRecord, Target, Version};

/// Installer starts per release before updating needs a manual install.
pub const MAX_ATTEMPTS: u32 = 3;

/// Whether `failed` starts of one release use up its automatic attempts.
pub(super) fn exhausted(failed: u32) -> bool {
    failed >= MAX_ATTEMPTS
}

/// The platform part of applying an update.
pub trait Installer {
    /// The installation directory `running` belongs to, if this installer
    /// owns it. A loose copy has none and never replaces itself.
    fn locate(&self, running: &Path) -> Option<PathBuf>;
    /// Whether that installation already holds the signed release.
    fn installed(&self, install: &Path, record: &ArtifactRecord) -> bool;
    /// The arguments that run the verified installer unattended for it.
    fn arguments(&self, install: &Path, log: &Path) -> Vec<OsString>;
    /// Starts the installer so that it outlives this process. `record` is
    /// the signed artifact `program` must still match when it starts.
    fn start(
        &self,
        program: &Path,
        record: &ArtifactRecord,
        arguments: &[OsString],
        log: &Path,
    ) -> io::Result<()>;
}

/// The installer for this platform, if updates can be applied here.
pub fn platform_installer() -> Option<Box<dyn Installer>> {
    #[cfg(target_os = "linux")]
    return super::linux::LinuxInstaller::from_environment()
        .map(|installer| Box::new(installer) as Box<dyn Installer>);
    #[cfg(windows)]
    return Some(Box::new(super::windows::WindowsInstaller));
    #[cfg(not(any(target_os = "linux", windows)))]
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// Updates are off; staging is cleared.
    Disabled,
    /// Updates cannot run in this build or another process owns the store.
    Unavailable,
    /// No verified release is pending.
    NothingPending,
    /// This process is not an installation the installer owns.
    Ineligible,
    /// The installer failed [`MAX_ATTEMPTS`] times for this release.
    GaveUp,
    AlreadyInstalled,
    /// The staged bytes no longer match; the next check downloads again.
    NotVerified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Skip(Skip),
    Start {
        program: PathBuf,
        /// Boxed: the signed row is large next to a skip reason.
        record: Box<ArtifactRecord>,
        arguments: Vec<OsString>,
        log: PathBuf,
        attempt: u32,
    },
}

/// Decides what to do at Quit without changing anything. Cheap checks come
/// before hashing the installed app and the staged artifact.
pub(super) fn plan(
    store: &Store,
    target: Target,
    installed: Version,
    running: &Path,
    installer: &dyn Installer,
) -> Plan {
    let Some(record) = store.pending(target, installed) else {
        return Plan::Skip(Skip::NothingPending);
    };
    let Some(install) = installer.locate(running) else {
        return Plan::Skip(Skip::Ineligible);
    };
    let version = record.version;
    let previous = store
        .journal()
        .filter(|journal| journal.version == version.to_string());
    if previous
        .as_ref()
        .is_some_and(|journal| exhausted(store.failed_starts(journal)))
    {
        return Plan::Skip(Skip::GaveUp);
    }
    let attempt = previous.map_or(1, |journal| journal.attempt + 1);
    if installer.installed(&install, &record) {
        return Plan::Skip(Skip::AlreadyInstalled);
    }
    let Some(program) = store.verified(&record) else {
        return Plan::Skip(Skip::NotVerified);
    };
    let log = store.apply_log(version, attempt);
    Plan::Start {
        arguments: installer.arguments(&install, &log),
        program,
        record: Box::new(record),
        log,
        attempt,
    }
}

/// Plans and carries out the Quit-time step: clears staging when updates are
/// off, or journals the attempt and then starts the installer.
pub(super) fn after_exit(
    store: &Store,
    enabled: bool,
    target: Target,
    installed: Version,
    running: &Path,
    installer: &dyn Installer,
) -> io::Result<Plan> {
    store.settle_journal(installed)?;
    store.remove_settled_reports(installed);
    if !enabled {
        store.clean_staging(None)?;
        return Ok(Plan::Skip(Skip::Disabled));
    }
    let plan = plan(store, target, installed, running, installer);
    if let Plan::Start {
        program,
        record,
        arguments,
        log,
        attempt,
    } = &plan
    {
        store.clear_refused(record.version, *attempt)?;
        store.save_journal(&Journal {
            version: record.version.to_string(),
            attempt: *attempt,
        })?;
        installer.start(program, record, arguments, log)?;
    }
    Ok(plan)
}

#[cfg(test)]
mod tests;
