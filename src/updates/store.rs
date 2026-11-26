//! Durable update floors and the artifact staging directory.
//!
//! The floors live in their own file, never in `config.toml`: the host
//! replaces the whole preferences file with the UI child's copy, which would
//! roll them back. The caller picks a persistent directory for the floors (for
//! example the local data directory) and a disposable one for staging (for
//! example the cache directory). Staging holds at most the current pending
//! artifact. One process at a time owns both, through an advisory lock file.
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{
    ArtifactRecord, ReleaseFloor, RootPolicy, State, Status, Target, Version, hex, to_hex, verify,
};

pub(super) const STATE_FILE: &str = "update-state.json";
const LOCK_FILE: &str = "update.lock";
/// The exact bytes of the accepted bundle; the stored floor digest binds them.
const BUNDLE_FILE: &str = "accepted-bundle.json";
const JOURNAL_FILE: &str = "apply-journal.json";
/// Installer reports are named `apply-<version>-<attempt>.log`.
const APPLY_LOG_PREFIX: &str = "apply-";
const APPLY_LOG_SUFFIX: &str = ".log";
/// A setup refused because another setup was running leaves
/// `apply-<version>-<attempt>.refused` (packaging/windows/setup.iss).
const REFUSED_SUFFIX: &str = ".refused";
const FORMAT: u32 = 1;

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    /// The state file exists but is not valid. It is never reset silently,
    /// because a reset would drop the rollback floors.
    Corrupt,
    /// A save would lower the stored root version or release sequence.
    Regression,
}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    format: u32,
    /// Canonical root policy JSON, kept byte-exact.
    root: String,
    release: Option<StoredRelease>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRelease {
    sequence: u64,
    version: String,
    digest: String,
    pending: bool,
}

/// The last installer run after Quit, so failures are counted and bounded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Journal {
    pub(super) version: String,
    /// The number of the last start; each start has its own report.
    pub(super) attempt: u32,
}

/// An installer report or refusal marker in the state folder.
struct Report {
    path: PathBuf,
    version: Version,
    attempt: u32,
    refused: bool,
}

impl Report {
    /// Parses `apply-<version>-<attempt>.log` or `.refused`.
    fn parse(path: PathBuf) -> Option<Self> {
        let name = path.file_name()?.to_str()?;
        let rest = name.strip_prefix(APPLY_LOG_PREFIX)?;
        let (rest, refused) = match rest.strip_suffix(REFUSED_SUFFIX) {
            Some(rest) => (rest, true),
            None => (rest.strip_suffix(APPLY_LOG_SUFFIX)?, false),
        };
        let (version, attempt) = rest.rsplit_once('-')?;
        Some(Self {
            version: Version::parse(version).ok()?,
            attempt: attempt.parse().ok()?,
            path,
            refused,
        })
    }
}

/// Replaces `path` atomically and durably: a crash leaves the old or the new
/// file, never a partial one, and a power cut cannot bring back the previous
/// file and with it lower floors.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    crate::fsutil::replace_atomically(path, bytes, None, || Ok(()))
}

pub struct Store {
    state: PathBuf,
    staging: PathBuf,
    _lock: File,
    /// Serializes checks within this process; the file lock covers others.
    pub(super) checking: tokio::sync::Mutex<()>,
}

impl Store {
    /// Per-user locations: floors under the local data directory and staging
    /// under the cache directory.
    pub fn default_dirs() -> Option<(PathBuf, PathBuf)> {
        let dirs = crate::config::project_dirs()?;
        Some((
            dirs.data_local_dir().join("updates"),
            dirs.cache_dir().join("updates"),
        ))
    }

    /// Takes the update lock, or returns `Ok(None)` while another process
    /// holds it. The lock is released when the store is dropped.
    pub fn open(state_dir: &Path, staging_dir: &Path) -> io::Result<Option<Self>> {
        fs::create_dir_all(state_dir)?;
        fs::create_dir_all(staging_dir)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(state_dir.join(LOCK_FILE))?;
        match lock.try_lock() {
            Ok(()) => Ok(Some(Self {
                state: state_dir.join(STATE_FILE),
                staging: staging_dir.to_owned(),
                _lock: lock,
                checking: tokio::sync::Mutex::new(()),
            })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error),
        }
    }

    /// Returns `Ok(None)` before the first save.
    pub fn load(&self) -> Result<Option<State>, StoreError> {
        let text = match fs::read_to_string(&self.state) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let stored: Stored = serde_json::from_str(&text).map_err(|_| StoreError::Corrupt)?;
        if stored.format != FORMAT {
            return Err(StoreError::Corrupt);
        }
        let root =
            RootPolicy::from_json(stored.root.as_bytes()).map_err(|_| StoreError::Corrupt)?;
        let release = match stored.release {
            None => None,
            Some(release) => Some(ReleaseFloor {
                sequence: release.sequence,
                version: Version::parse(&release.version).map_err(|_| StoreError::Corrupt)?,
                digest: hex::<32>(&release.digest).ok_or(StoreError::Corrupt)?,
                pending: release.pending,
            }),
        };
        Ok(Some(State { root, release }))
    }

    /// Replaces the state file atomically. Floors never go down, and a
    /// corrupt file is never overwritten.
    pub(super) fn save(&self, state: &State) -> Result<(), StoreError> {
        if let Some(stored) = self.load()? {
            let sequence = |state: &State| state.release.map(|release| release.sequence);
            if state.root.version() < stored.root.version() || sequence(state) < sequence(&stored) {
                return Err(StoreError::Regression);
            }
        }
        let stored = Stored {
            format: FORMAT,
            root: String::from_utf8(state.root.canonical_json().to_vec())
                .expect("canonical JSON is ASCII"),
            release: state.release.map(|release| StoredRelease {
                sequence: release.sequence,
                version: release.version.to_string(),
                digest: to_hex(&release.digest),
                pending: release.pending,
            }),
        };
        let bytes = serde_json::to_vec(&stored).map_err(io::Error::from)?;
        write_atomic(&self.state, &bytes)?;
        Ok(())
    }

    fn state_file(&self, name: &str) -> PathBuf {
        self.state.with_file_name(name)
    }

    /// Keeps the accepted bundle, so a later session can re-verify the
    /// pending artifact without the network. Identical bytes are not
    /// rewritten.
    pub(super) fn save_bundle(&self, raw: &[u8]) -> io::Result<()> {
        let path = self.state_file(BUNDLE_FILE);
        if fs::read(&path).is_ok_and(|stored| stored == raw) {
            return Ok(());
        }
        write_atomic(&path, raw)
    }

    /// The signed record of the pending release, verified again offline from
    /// the stored bundle against the stored floors.
    pub(super) fn pending(&self, target: Target, installed: Version) -> Option<ArtifactRecord> {
        let state = self.load().ok()??;
        let release = state.release.filter(|release| release.pending)?;
        let raw = fs::read(self.state_file(BUNDLE_FILE)).ok()?;
        let accepted = verify(&raw, &state, target, installed, release.version)
            .result
            .ok()?;
        (accepted.status == Status::Retry).then_some(accepted.artifact)
    }

    pub(super) fn journal(&self) -> Option<Journal> {
        let text = fs::read(self.state_file(JOURNAL_FILE)).ok()?;
        serde_json::from_slice(&text).ok()
    }

    /// The starts of the journaled release that failed: every start up to
    /// the journaled one except those another running setup refused.
    pub(super) fn failed_starts(&self, journal: &Journal) -> u32 {
        let version = Version::parse(&journal.version).ok();
        let refused = self
            .reports()
            .filter(|report| {
                report.refused
                    && Some(report.version) == version
                    && report.attempt <= journal.attempt
            })
            .count();
        journal.attempt.saturating_sub(refused as u32)
    }

    /// Removes a refusal marker left with this start number, for example
    /// after the journal was lost, so the new start is not taken as refused.
    pub(super) fn clear_refused(&self, version: Version, attempt: u32) -> io::Result<()> {
        let marker = format!("{APPLY_LOG_PREFIX}{version}-{attempt}{REFUSED_SUFFIX}");
        match fs::remove_file(self.state_file(&marker)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    fn reports(&self) -> impl Iterator<Item = Report> {
        let directory = self.state.parent().expect("store files have a directory");
        fs::read_dir(directory)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter_map(|entry| Report::parse(entry.path()))
    }

    pub(super) fn save_journal(&self, journal: &Journal) -> io::Result<()> {
        let bytes = serde_json::to_vec(journal).map_err(io::Error::from)?;
        write_atomic(&self.state_file(JOURNAL_FILE), &bytes)
    }

    /// Forgets the journal once the installed version has reached it.
    pub(super) fn settle_journal(&self, installed: Version) -> io::Result<()> {
        let reached = self.journal().is_some_and(|journal| {
            Version::parse(&journal.version).map_or(true, |version| version <= installed)
        });
        if reached {
            fs::remove_file(self.state_file(JOURNAL_FILE))?;
        }
        Ok(())
    }

    /// Where one installer start writes its report: beside the floors, so a
    /// staging cleanup does not remove it, and one file per attempt, so a
    /// retry keeps the report of the attempt that failed.
    pub(super) fn apply_log(&self, version: Version, attempt: u32) -> PathBuf {
        self.state_file(&format!(
            "{APPLY_LOG_PREFIX}{version}-{attempt}{APPLY_LOG_SUFFIX}"
        ))
    }

    /// Removes the reports of versions up to the installed one, as far as
    /// possible. A report that cannot be removed now, for example because a
    /// viewer holds it open, never blocks the step and goes at a later Quit.
    pub(super) fn remove_settled_reports(&self, installed: Version) {
        for report in self.reports() {
            if report.version <= installed {
                let _ = fs::remove_file(report.path);
            }
        }
    }

    /// The staged artifact for `record`, if its bytes still match the signed
    /// size and SHA-256. Call it again just before handing the file over.
    pub fn verified(&self, record: &ArtifactRecord) -> Option<PathBuf> {
        let path = self.staging.join(&record.filename);
        record.matches(File::open(&path).ok()?).then_some(path)
    }

    /// Starts staging `record`: download into [`Staging::partial`], then
    /// [`Staging::commit`]. Dropping it uncommitted removes the partial file.
    /// The filename was checked against the constructed contract name, so it
    /// cannot leave the directory.
    pub(super) fn stage(&self, record: &ArtifactRecord) -> Staging {
        Staging {
            partial: self.staging.join(format!("{}.partial", record.filename)),
            path: Some(self.staging.join(&record.filename)),
        }
    }

    /// Removes every staging entry except the file named `keep`.
    pub(super) fn clean_staging(&self, keep: Option<&str>) -> io::Result<()> {
        for entry in fs::read_dir(&self.staging)? {
            let entry = entry?;
            if keep.is_some_and(|keep| entry.file_name() == keep) {
                continue;
            }
            if entry.file_type()?.is_dir() {
                fs::remove_dir_all(entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }
}

pub(super) struct Staging {
    partial: PathBuf,
    /// `None` once committed.
    path: Option<PathBuf>,
}

impl Staging {
    pub(super) fn partial(&self) -> &Path {
        &self.partial
    }

    /// Moves the verified download into place and returns its path.
    pub(super) fn commit(mut self) -> io::Result<PathBuf> {
        let path = self.path.take().expect("staging commits once");
        fs::rename(&self.partial, &path)?;
        Ok(path)
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if self.path.is_some() {
            let _ = fs::remove_file(&self.partial);
        }
    }
}

/// A scanner or an editor can hold a store file open (#67).
#[cfg(all(test, windows))]
mod open_target_tests {
    use super::*;

    #[test]
    fn journal_save_replaces_a_file_a_reader_holds_open() {
        let state = tempfile::tempdir().unwrap();
        let staging = tempfile::tempdir().unwrap();
        let store = Store::open(state.path(), staging.path()).unwrap().unwrap();
        let first = Journal {
            version: "0.2.0".into(),
            attempt: 1,
        };
        store.save_journal(&first).unwrap();
        let _reader = File::open(store.state_file(JOURNAL_FILE)).unwrap();
        let next = Journal {
            version: "0.2.0".into(),
            attempt: 2,
        };
        store.save_journal(&next).unwrap();
        assert_eq!(store.journal(), Some(next));
    }
}
