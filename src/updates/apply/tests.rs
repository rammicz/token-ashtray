//! The Quit-time step against a store staged through the local HTTP fixture.
use std::cell::RefCell;
use std::fs;
use std::time::Duration;

use tempfile::TempDir;

use super::*;
use crate::updates::tests::{example, payload, reopen, root_v1, v};
use crate::updates::transport::tests::{ARTIFACT, LATEST, Server, TAGGED, ok, redirect};
use crate::updates::{Outcome, check_once};

const INSTALLED: &str = "0.2.1";

#[derive(Default)]
struct FakeInstaller {
    ineligible: bool,
    installed: bool,
    starts: RefCell<Vec<(PathBuf, Vec<OsString>, PathBuf)>>,
}

impl Installer for FakeInstaller {
    fn locate(&self, running: &Path) -> Option<PathBuf> {
        (!self.ineligible).then(|| running.parent().unwrap().to_owned())
    }

    fn installed(&self, _install: &Path, _record: &ArtifactRecord) -> bool {
        self.installed
    }

    fn arguments(&self, install: &Path, log: &Path) -> Vec<OsString> {
        vec![install.as_os_str().to_owned(), log.as_os_str().to_owned()]
    }

    fn start(
        &self,
        program: &Path,
        _record: &ArtifactRecord,
        arguments: &[OsString],
        log: &Path,
    ) -> io::Result<()> {
        self.starts
            .borrow_mut()
            .push((program.to_owned(), arguments.to_vec(), log.to_owned()));
        Ok(())
    }
}

fn open(dirs: &TempDir) -> Store {
    let (state, staging) = (dirs.path().join("data"), dirs.path().join("cache"));
    reopen(&state, &staging)
}

struct Staged {
    dirs: TempDir,
    path: PathBuf,
    record: ArtifactRecord,
}

impl Staged {
    /// Downloads and stages release 0.2.2 in a fresh store, then closes it as
    /// if the app had restarted.
    async fn new() -> Self {
        Self::for_target(Target::LinuxX86_64).await
    }

    async fn for_target(target: Target) -> Self {
        let dirs = tempfile::tempdir().unwrap();
        let artifact = ARTIFACT.replace(
            &Target::LinuxX86_64.artifact_name(v("0.2.2")),
            &target.artifact_name(v("0.2.2")),
        );
        let server = Server::new(&[
            (LATEST, vec![redirect(TAGGED)]),
            (TAGGED, vec![ok(&example("0.2.2", 2))]),
            (&artifact, vec![ok(&payload(target, "0.2.2"))]),
        ]);
        let transport = server.transport(Duration::from_secs(5));
        let store = open(&dirs);
        let outcome = check_once(&store, &transport, &root_v1(), target, v(INSTALLED))
            .await
            .unwrap();
        let Outcome::Staged { path, record } = outcome else {
            panic!("expected a staged release");
        };
        Self { dirs, path, record }
    }

    fn open(&self) -> Store {
        open(&self.dirs)
    }

    fn quit(&self, enabled: bool, installed: &str, installer: &FakeInstaller) -> Plan {
        let store = self.open();
        after_exit(
            &store,
            enabled,
            Target::LinuxX86_64,
            v(installed),
            Path::new("/opt/token-ashtray/token-ashtray"),
            installer,
        )
        .unwrap()
    }
}

#[tokio::test]
async fn quit_after_a_restart_hands_the_reverified_artifact_to_the_installer() {
    let staged = Staged::new().await;
    let installer = FakeInstaller::default();
    let plan = staged.quit(true, INSTALLED, &installer);
    let log = staged.dirs.path().join("data").join("apply-0.2.2-1.log");
    let arguments = vec!["/opt/token-ashtray".into(), log.clone().into_os_string()];
    let expected = Plan::Start {
        program: staged.path.clone(),
        record: Box::new(staged.record.clone()),
        arguments: arguments.clone(),
        log: log.clone(),
        attempt: 1,
    };
    assert_eq!(plan, expected);
    assert_eq!(
        *installer.starts.borrow(),
        [(staged.path.clone(), arguments, log)]
    );
    let journal = staged.open().journal().unwrap();
    assert_eq!((journal.version.as_str(), journal.attempt), ("0.2.2", 1));
}

#[tokio::test]
async fn disabled_updates_clear_staging_and_start_nothing() {
    let staged = Staged::new().await;
    let installer = FakeInstaller::default();
    assert_eq!(
        staged.quit(false, INSTALLED, &installer),
        Plan::Skip(Skip::Disabled)
    );
    assert!(!staged.path.exists());
    assert!(installer.starts.borrow().is_empty());
}

#[tokio::test]
async fn loose_copies_installed_releases_and_changed_bytes_are_skipped() {
    let staged = Staged::new().await;
    let loose = FakeInstaller {
        ineligible: true,
        ..Default::default()
    };
    assert_eq!(
        staged.quit(true, INSTALLED, &loose),
        Plan::Skip(Skip::Ineligible)
    );
    let current = FakeInstaller {
        installed: true,
        ..Default::default()
    };
    assert_eq!(
        staged.quit(true, INSTALLED, &current),
        Plan::Skip(Skip::AlreadyInstalled)
    );
    let installer = FakeInstaller::default();
    let mut bytes = fs::read(&staged.path).unwrap();
    bytes[0] ^= 1;
    fs::write(&staged.path, bytes).unwrap();
    assert_eq!(
        staged.quit(true, INSTALLED, &installer),
        Plan::Skip(Skip::NotVerified)
    );
    // A changed stored bundle no longer proves which artifact is pending.
    let bundle = staged.dirs.path().join("data").join("accepted-bundle.json");
    fs::write(&bundle, b"{}").unwrap();
    assert_eq!(
        staged.quit(true, INSTALLED, &installer),
        Plan::Skip(Skip::NothingPending)
    );
    for skipped in [&loose, &current, &installer] {
        assert!(skipped.starts.borrow().is_empty());
    }
}

#[tokio::test]
async fn a_failing_installer_is_retried_a_bounded_number_of_times() {
    let staged = Staged::new().await;
    let installer = FakeInstaller::default();
    for attempt in 1..=MAX_ATTEMPTS {
        let Plan::Start {
            attempt: started, ..
        } = staged.quit(true, INSTALLED, &installer)
        else {
            panic!("attempt {attempt} must start the installer");
        };
        assert_eq!(started, attempt);
    }
    assert_eq!(
        staged.quit(true, INSTALLED, &installer),
        Plan::Skip(Skip::GaveUp)
    );
    assert_eq!(installer.starts.borrow().len(), MAX_ATTEMPTS as usize);
}

#[tokio::test]
async fn reaching_the_journaled_version_settles_the_journal() {
    let staged = Staged::new().await;
    let installer = FakeInstaller::default();
    assert!(matches!(
        staged.quit(true, INSTALLED, &installer),
        Plan::Start { .. }
    ));
    // The next session runs the new version: nothing is pending any more.
    assert_eq!(
        staged.quit(true, "0.2.2", &installer),
        Plan::Skip(Skip::NothingPending)
    );
    assert!(staged.open().journal().is_none());
}

#[tokio::test]
async fn each_attempt_keeps_its_report_until_the_update_settles() {
    let staged = Staged::new().await;
    let installer = FakeInstaller::default();
    let data = staged.dirs.path().join("data");
    let mut reports = Vec::new();
    for attempt in 1..=2 {
        let Plan::Start { log, .. } = staged.quit(true, INSTALLED, &installer) else {
            panic!("attempt {attempt} must start the installer");
        };
        assert_eq!(log, data.join(format!("apply-0.2.2-{attempt}.log")));
        // The installer writes its report; a failed attempt leaves it behind.
        fs::write(&log, format!("attempt {attempt}")).unwrap();
        reports.push(log);
    }
    assert!(reports.iter().all(|report| report.exists()));
    staged.quit(true, "0.2.2", &installer);
    assert!(reports.iter().all(|report| !report.exists()));
    // Only the reports go; the floors and the accepted bundle stay.
    assert!(data.join("update-state.json").exists());
    assert!(data.join("accepted-bundle.json").exists());
}

#[tokio::test]
async fn markers_left_from_a_lost_journal_do_not_excuse_new_failures() {
    let staged = Staged::new().await;
    let installer = FakeInstaller::default();
    let data = staged.dirs.path().join("data");
    // The journal is gone, but markers of earlier starts of this release
    // remain, one with the number the next start reuses.
    for name in ["apply-0.2.2-1.refused", "apply-0.2.2-5.refused"] {
        fs::write(data.join(name), "another setup was running").unwrap();
    }
    assert!(staged.open().journal().is_none());
    for attempt in 1..=MAX_ATTEMPTS {
        let Plan::Start {
            attempt: started, ..
        } = staged.quit(true, INSTALLED, &installer)
        else {
            panic!("attempt {attempt} must start the installer");
        };
        assert_eq!(started, attempt);
        // The start reusing a refused number is not refused.
        assert!(!data.join(format!("apply-0.2.2-{attempt}.refused")).exists());
    }
    assert_eq!(
        staged.quit(true, INSTALLED, &installer),
        Plan::Skip(Skip::GaveUp)
    );
}

#[tokio::test]
async fn a_start_refused_by_a_running_setup_is_not_a_failed_attempt() {
    let staged = Staged::new().await;
    let installer = FakeInstaller::default();
    let data = staged.dirs.path().join("data");
    // One start is refused because another setup was running; the setup
    // leaves a marker beside the report of that start.
    let Plan::Start { log, .. } = staged.quit(true, INSTALLED, &installer) else {
        panic!("the first Quit must start the installer");
    };
    fs::write(log.with_extension("refused"), "another setup was running").unwrap();
    // The bound still allows MAX_ATTEMPTS starts that really failed, each
    // with a report of its own.
    for attempt in 2..=MAX_ATTEMPTS + 1 {
        let Plan::Start { log, .. } = staged.quit(true, INSTALLED, &installer) else {
            panic!("start {attempt} must run: only real failures count");
        };
        assert_eq!(log, data.join(format!("apply-0.2.2-{attempt}.log")));
    }
    assert_eq!(
        staged.quit(true, INSTALLED, &installer),
        Plan::Skip(Skip::GaveUp)
    );
    let store = staged.open();
    let journal = store.journal().unwrap();
    assert_eq!(
        (journal.attempt, store.failed_starts(&journal)),
        (MAX_ATTEMPTS + 1, MAX_ATTEMPTS)
    );
}

#[tokio::test]
async fn reports_of_settled_versions_are_cleaned_at_every_quit() {
    let staged = Staged::new().await;
    let installer = FakeInstaller::default();
    let data = staged.dirs.path().join("data");
    let stale = [
        "apply-0.2.0-1.log",
        "apply-0.2.0-2.refused",
        "apply-0.2.1-3.log",
    ];
    for name in stale {
        fs::write(data.join(name), name).unwrap();
    }
    // Reports of the pending release and unrelated files stay.
    let kept = [
        "apply-0.2.2-1.log",
        "apply-0.2.2-1.refused",
        "apply-notes.log",
    ];
    for name in kept {
        fs::write(data.join(name), name).unwrap();
    }
    // No journal is needed: the reports of versions up to the running one
    // are removed whenever the step runs, so a failed removal is retried.
    assert!(staged.open().journal().is_none());
    staged.quit(false, INSTALLED, &installer);
    for name in stale {
        assert!(!data.join(name).exists(), "{name} must be removed");
    }
    for name in kept {
        assert!(data.join(name).exists(), "{name} must stay");
    }
}

/// A report that cannot be removed now never blocks the step, and is removed
/// at a later Quit.
#[cfg(windows)]
#[tokio::test]
async fn a_report_in_use_does_not_block_the_step_and_goes_later() {
    use std::os::windows::fs::OpenOptionsExt;
    let staged = Staged::new().await;
    let installer = FakeInstaller::default();
    let report = staged.dirs.path().join("data").join("apply-0.2.1-1.log");
    fs::write(&report, "kept open by a reader").unwrap();
    // The update to the running version has just settled.
    let journal = Journal {
        version: INSTALLED.into(),
        attempt: 1,
    };
    staged.open().save_journal(&journal).unwrap();
    // Open for reading only, without delete sharing, as a log viewer might.
    let reader = fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1)
        .open(&report)
        .unwrap();
    assert!(matches!(
        staged.quit(true, INSTALLED, &installer),
        Plan::Start { .. }
    ));
    assert!(report.exists());
    drop(reader);
    staged.quit(true, INSTALLED, &installer);
    assert!(!report.exists());
}

/// Windows, with the platform installer this build returns.
#[cfg(windows)]
mod windows {
    use super::*;

    const RUNNING: &str = "token-ashtray.exe";
    const SILENT: [&str; 5] = [
        "/VERYSILENT",
        "/SUPPRESSMSGBOXES",
        "/NORESTART",
        "/NOCANCEL",
        "/SP-",
    ];

    /// An installation made by the graphical setup, in a folder with a space:
    /// the app, its marker and the setup resources marker, with exact texts.
    fn setup_install(root: &Path) -> PathBuf {
        let install = root.join("Token Ashtray");
        fs::create_dir_all(install.join(".setup")).unwrap();
        fs::write(install.join(RUNNING), b"MZ installed 0.2.1").unwrap();
        fs::write(
            install.join(".token-ashtray-install"),
            "Token Ashtray per-user installation\n",
        )
        .unwrap();
        fs::write(
            install.join(".setup").join(".token-ashtray-setup"),
            "Token Ashtray graphical setup resources\n",
        )
        .unwrap();
        install
    }

    fn installer() -> Box<dyn Installer> {
        platform_installer().expect("Windows applies verified updates")
    }

    fn plan_for(staged: &Staged, running: &Path) -> Plan {
        let store = staged.open();
        plan(
            &store,
            Target::WindowsX86_64,
            v(INSTALLED),
            running,
            installer().as_ref(),
        )
    }

    #[tokio::test]
    async fn windows_quit_hands_verified_setup_to_installer_with_install_dir() {
        let staged = Staged::for_target(Target::WindowsX86_64).await;
        let install = setup_install(staged.dirs.path());
        let log = staged.dirs.path().join("data").join("apply-0.2.2-1.log");
        let mut arguments: Vec<OsString> = SILENT.iter().map(OsString::from).collect();
        arguments.push(format!("/DIR=\"{}\"", install.display()).into());
        arguments.push(format!("/LOG=\"{}\"", log.display()).into());
        arguments.push("/WAITAPP".into());
        assert_eq!(
            plan_for(&staged, &install.join(RUNNING)),
            Plan::Start {
                program: staged.path.clone(),
                record: Box::new(staged.record.clone()),
                arguments,
                log,
                attempt: 1,
            }
        );
        // The journal comes first, so a start that fails still counts. The
        // synthetic staged bytes are not a program, so Windows refuses them.
        let store = staged.open();
        let started = after_exit(
            &store,
            true,
            Target::WindowsX86_64,
            v(INSTALLED),
            &install.join(RUNNING),
            installer().as_ref(),
        );
        assert!(
            started.is_err(),
            "synthetic bytes must not run: {started:?}"
        );
        let journal = store.journal().unwrap();
        assert_eq!((journal.version.as_str(), journal.attempt), ("0.2.2", 1));
    }

    #[tokio::test]
    async fn windows_loose_copies_and_other_installations_never_apply() {
        let staged = Staged::for_target(Target::WindowsX86_64).await;
        let install = setup_install(staged.dirs.path());
        let loose = staged.dirs.path().join("Downloads");
        fs::create_dir_all(&loose).unwrap();
        fs::write(loose.join(RUNNING), b"MZ loose").unwrap();
        assert_eq!(
            plan_for(&staged, &loose.join(RUNNING)),
            Plan::Skip(Skip::Ineligible)
        );
        // Another program in the installation folder is not the app.
        assert_eq!(
            plan_for(&staged, &install.join("other.exe")),
            Plan::Skip(Skip::Ineligible)
        );
        // A helper-only installation has no setup resources to keep coherent.
        let setup_marker = install.join(".setup").join(".token-ashtray-setup");
        fs::remove_file(&setup_marker).unwrap();
        assert_eq!(
            plan_for(&staged, &install.join(RUNNING)),
            Plan::Skip(Skip::Ineligible)
        );
        fs::write(&setup_marker, "Token Ashtray graphical setup resources\r\n").unwrap();
        assert_eq!(
            plan_for(&staged, &install.join(RUNNING)),
            Plan::Skip(Skip::Ineligible)
        );
        fs::write(&setup_marker, "Token Ashtray graphical setup resources\n").unwrap();
        fs::write(install.join(".token-ashtray-install"), "something else\n").unwrap();
        assert_eq!(
            plan_for(&staged, &install.join(RUNNING)),
            Plan::Skip(Skip::Ineligible)
        );
    }

    #[tokio::test]
    async fn windows_disabled_changed_bytes_and_exhausted_attempts_start_nothing() {
        let staged = Staged::for_target(Target::WindowsX86_64).await;
        let install = setup_install(staged.dirs.path());
        let running = install.join(RUNNING);
        let store = staged.open();
        store
            .save_journal(&Journal {
                version: "0.2.2".into(),
                attempt: MAX_ATTEMPTS,
            })
            .unwrap();
        drop(store);
        assert_eq!(plan_for(&staged, &running), Plan::Skip(Skip::GaveUp));
        fs::remove_file(staged.dirs.path().join("data").join("apply-journal.json")).unwrap();
        let mut bytes = fs::read(&staged.path).unwrap();
        bytes[0] ^= 1;
        fs::write(&staged.path, bytes).unwrap();
        assert_eq!(plan_for(&staged, &running), Plan::Skip(Skip::NotVerified));
        let store = staged.open();
        let disabled = after_exit(
            &store,
            false,
            Target::WindowsX86_64,
            v(INSTALLED),
            &running,
            installer().as_ref(),
        )
        .unwrap();
        assert_eq!(disabled, Plan::Skip(Skip::Disabled));
        assert!(!staged.path.exists());
        assert!(store.journal().is_none());
    }
}
