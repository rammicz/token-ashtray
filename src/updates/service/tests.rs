//! The service against the local HTTP fixture: no request before opt-in.
use std::fs;
use std::thread;
use std::time::Instant;

use tempfile::TempDir;

use super::*;
use crate::updates::tests::{example, payload, root_v1, v};
#[cfg(not(feature = "qa-offline-update"))]
use crate::updates::transport::tests::path;
use crate::updates::transport::tests::{ARTIFACT, LATEST, Server, TAGGED, delayed, ok, redirect};

#[cfg(not(feature = "qa-offline-update"))]
const QUIET: Duration = Duration::from_millis(300);

fn feed(artifact_delay: Duration) -> Server {
    Server::new(&[
        (LATEST, vec![redirect(TAGGED)]),
        (TAGGED, vec![ok(&example("0.2.2", 2))]),
        (
            ARTIFACT,
            vec![delayed(
                &payload(Target::LinuxX86_64, "0.2.2"),
                artifact_delay,
            )],
        ),
    ])
}

fn config(server: &Server, dirs: &TempDir, bootstrap: Option<RootPolicy>) -> UpdaterConfig {
    UpdaterConfig {
        dirs: Some((dirs.path().join("data"), dirs.path().join("cache"))),
        bootstrap,
        target: Some(Target::LinuxX86_64),
        installed: v("0.2.1"),
        transport: Some(server.transport(Duration::from_secs(5))),
    }
}

fn start(config: UpdaterConfig, enabled: bool) -> Updater {
    Updater::start(&Primary::unguarded(), config, enabled, Arc::new(|| {}))
}

/// Polls `condition` for up to ten seconds.
fn until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out: {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for(updater: &Updater, expected: &UpdateStatus) {
    until(&format!("status {expected:?}"), || {
        updater.status() == *expected
    });
}

/// Production behaviour: QA builds check offline (#52).
#[cfg(not(feature = "qa-offline-update"))]
#[test]
fn no_request_before_opt_in_and_none_after_opting_out() {
    let dirs = tempfile::tempdir().unwrap();
    let server = feed(Duration::ZERO);
    let updater = start(config(&server, &dirs, Some(root_v1())), false);
    thread::sleep(QUIET);
    assert_eq!(updater.status(), UpdateStatus::Off);
    assert!(server.paths().is_empty(), "{:?}", server.paths());

    updater.set_enabled(true);
    wait_for(&updater, &UpdateStatus::Ready("0.2.2".into()));
    assert!(server.paths().contains(&path(ARTIFACT)));

    updater.set_enabled(false);
    wait_for(&updater, &UpdateStatus::Off);
    let requests = server.paths().len();
    thread::sleep(QUIET);
    assert_eq!(server.paths().len(), requests);
}

/// Production behaviour: QA builds check offline (#52).
#[cfg(not(feature = "qa-offline-update"))]
#[test]
fn opting_out_cancels_a_download_in_flight() {
    let dirs = tempfile::tempdir().unwrap();
    let server = feed(Duration::from_secs(3));
    let updater = start(config(&server, &dirs, Some(root_v1())), true);
    until("the download starts", || {
        server.paths().contains(&path(ARTIFACT))
    });
    let disabled = Instant::now();
    updater.set_enabled(false);
    wait_for(&updater, &UpdateStatus::Off);
    assert!(disabled.elapsed() < Duration::from_secs(2));
    // The cancelled download leaves no partial or staged file behind.
    let staging = dirs.path().join("cache");
    until("staging drops the cancelled download", || {
        fs::read_dir(&staging).unwrap().count() == 0
    });
}

/// Production behaviour: QA builds check offline (#52).
#[cfg(not(feature = "qa-offline-update"))]
#[test]
fn without_an_embedded_root_updates_are_unavailable_and_silent() {
    assert!(embedded_root().is_none());
    for config in [UpdaterConfig::production(), UpdaterConfig::inert()] {
        assert!(matches!(config.ready(), Err(Unavailable::NoTrustRoot)));
    }
    let dirs = tempfile::tempdir().unwrap();
    let server = feed(Duration::ZERO);
    let updater = start(config(&server, &dirs, None), true);
    wait_for(
        &updater,
        &UpdateStatus::Unavailable(Unavailable::NoTrustRoot),
    );
    thread::sleep(QUIET);
    assert!(server.paths().is_empty(), "{:?}", server.paths());
}

/// Production behaviour: QA builds check offline (#52).
#[cfg(not(feature = "qa-offline-update"))]
#[test]
fn held_lock_and_corrupt_state_are_unavailable_without_requests() {
    let dirs = tempfile::tempdir().unwrap();
    let server = feed(Duration::ZERO);
    let (state, staging) = (dirs.path().join("data"), dirs.path().join("cache"));
    let held = Store::open(&state, &staging).unwrap().unwrap();
    let updater = start(config(&server, &dirs, Some(root_v1())), true);
    wait_for(
        &updater,
        &UpdateStatus::Unavailable(Unavailable::LockedElsewhere),
    );
    drop((updater, held));

    fs::write(state.join(crate::updates::store::STATE_FILE), b"{}").unwrap();
    let updater = start(config(&server, &dirs, Some(root_v1())), true);
    wait_for(
        &updater,
        &UpdateStatus::Unavailable(Unavailable::CorruptState),
    );
    assert!(server.paths().is_empty(), "{:?}", server.paths());
}

/// Fails the test if the Quit step tries to install anything.
struct NoInstaller;

impl crate::updates::apply::Installer for NoInstaller {
    fn locate(&self, _running: &Path) -> Option<PathBuf> {
        panic!("nothing may be located without an opt-in")
    }

    fn installed(&self, _install: &Path, _record: &crate::updates::ArtifactRecord) -> bool {
        unreachable!()
    }

    fn arguments(&self, _install: &Path, _log: &Path) -> Vec<std::ffi::OsString> {
        unreachable!()
    }

    fn start(
        &self,
        _: &Path,
        _: &crate::updates::ArtifactRecord,
        _: &[std::ffi::OsString],
        _: &Path,
    ) -> io::Result<()> {
        unreachable!()
    }
}

#[test]
fn quitting_without_opting_in_creates_no_update_files() {
    let dirs = tempfile::tempdir().unwrap();
    let server = feed(Duration::ZERO);
    let updater = start(config(&server, &dirs, Some(root_v1())), false);
    let plan = updater
        .after_exit(&NoInstaller, Path::new("/opt/token-ashtray"))
        .unwrap();
    assert_eq!(plan, Plan::Skip(apply::Skip::Disabled));
    assert!(!dirs.path().join("data").exists());
    assert!(!dirs.path().join("cache").exists());
    assert!(server.paths().is_empty());
}

/// Writes the journal an installer start after the last Quit left.
fn journal(dirs: &TempDir, version: &str, attempt: u32) {
    let state = dirs.path().join("data");
    fs::create_dir_all(&state).unwrap();
    let journal = serde_json::json!({"version": version, "attempt": attempt});
    fs::write(state.join("apply-journal.json"), journal.to_string()).unwrap();
}

/// Production behaviour: QA builds check offline (#52).
#[cfg(not(feature = "qa-offline-update"))]
#[test]
fn a_failed_install_is_reported_with_its_attempt_until_attempts_run_out() {
    for (attempt, expected) in [
        (
            1,
            UpdateStatus::InstallFailed {
                version: "0.2.2".into(),
                attempt: 1,
            },
        ),
        (
            apply::MAX_ATTEMPTS,
            UpdateStatus::InstallStopped("0.2.2".into()),
        ),
    ] {
        let dirs = tempfile::tempdir().unwrap();
        journal(&dirs, "0.2.2", attempt);
        let server = feed(Duration::ZERO);
        let updater = start(config(&server, &dirs, Some(root_v1())), true);
        wait_for(&updater, &expected);
    }
}

/// A start refused because another setup was running is not a failure.
#[cfg(not(feature = "qa-offline-update"))]
#[test]
fn a_refused_start_is_not_counted_as_a_failed_attempt() {
    let dirs = tempfile::tempdir().unwrap();
    journal(&dirs, "0.2.2", 2);
    let data = dirs.path().join("data");
    fs::write(
        data.join("apply-0.2.2-1.refused"),
        "another setup was running",
    )
    .unwrap();
    let server = feed(Duration::ZERO);
    let updater = start(config(&server, &dirs, Some(root_v1())), true);
    wait_for(
        &updater,
        &UpdateStatus::InstallFailed {
            version: "0.2.2".into(),
            attempt: 1,
        },
    );
}

#[test]
fn reaching_the_journaled_version_reports_the_update_once_up_to_date() {
    let dirs = tempfile::tempdir().unwrap();
    journal(&dirs, "0.2.2", 1);
    let server = feed(Duration::ZERO);
    let mut config = config(&server, &dirs, Some(root_v1()));
    config.installed = v("0.2.2");
    let updater = start(config, true);
    wait_for(&updater, &UpdateStatus::Installed("0.2.2".into()));
}

#[test]
fn the_last_install_only_explains_the_matching_check_result() {
    let last = |version: &str, failed| {
        let journal = Journal {
            version: version.into(),
            attempt: 2,
        };
        Some((journal, failed))
    };
    let journal = |version: &str| last(version, 2);
    // Starts that another running setup refused are not failures.
    let ready = UpdateStatus::Ready("0.2.2".into());
    assert_eq!(explain(ready.clone(), last("0.2.2", 0), v("0.2.1")), ready);
    // A newer release is a new download, not the failed one.
    let newer = UpdateStatus::Ready("0.2.3".into());
    assert_eq!(explain(newer.clone(), journal("0.2.2"), v("0.2.1")), newer);
    // A journal below the running version says nothing about this run.
    assert_eq!(
        explain(UpdateStatus::UpToDate, journal("0.2.2"), v("0.2.3")),
        UpdateStatus::UpToDate
    );
    for status in [
        UpdateStatus::Off,
        UpdateStatus::Checking,
        UpdateStatus::Failed(Failure::Network),
    ] {
        for installed in ["0.2.1", "0.2.2"] {
            assert_eq!(
                explain(status.clone(), journal("0.2.2"), v(installed)),
                status
            );
        }
    }
    assert_eq!(
        explain(UpdateStatus::UpToDate, None, v("0.2.2")),
        UpdateStatus::UpToDate
    );
}

#[cfg(not(feature = "qa-offline-update"))]
#[test]
fn release_builds_embed_no_release_root() {
    assert!(embedded_root().is_none());
}
