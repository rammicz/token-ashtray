//! The QA seed against temporary update folders.
use super::*;

fn v(text: &str) -> Version {
    Version::parse(text).unwrap()
}

struct Seeded {
    dirs: tempfile::TempDir,
    setup: PathBuf,
    app: PathBuf,
}

impl Seeded {
    fn new() -> Self {
        let dirs = tempfile::tempdir().unwrap();
        let (setup, app) = (dirs.path().join("Setup.exe"), dirs.path().join("app.exe"));
        fs::write(&setup, b"MZ setup 0.2.2").unwrap();
        fs::write(&app, b"MZ app 0.2.2").unwrap();
        Self { dirs, setup, app }
    }

    fn store(&self) -> Store {
        let path = self.dirs.path();
        Store::open(&path.join("data"), &path.join("cache"))
            .unwrap()
            .unwrap()
    }

    fn seed(&self, installed: &str, version: &str) -> Result<PathBuf, String> {
        let store = self.store();
        seed_into(
            &store,
            Target::WindowsX86_64,
            v(installed),
            v(version),
            &self.setup,
            &self.app,
        )
    }
}

#[test]
fn a_seeded_setup_is_pending_ready_and_verified_again_offline() {
    let seeded = Seeded::new();
    let staged = seeded.seed("0.2.1", "0.2.2").unwrap();
    assert_eq!(
        staged.file_name().unwrap(),
        "token-ashtray-setup-0.2.2-x86_64-pc-windows-msvc.exe"
    );
    let store = seeded.store();
    let record = store.pending(Target::WindowsX86_64, v("0.2.1")).unwrap();
    assert_eq!(record.app_sha256, sha256(b"MZ app 0.2.2"));
    assert_eq!(store.verified(&record), Some(staged));
    assert_eq!(
        offline_check(&store, Target::WindowsX86_64, v("0.2.1")),
        UpdateStatus::Ready("0.2.2".into())
    );
    drop(store);
    // Seeding the same release again is the exact retry the channel allows.
    seeded.seed("0.2.1", "0.2.2").unwrap();
}

#[test]
fn an_installed_release_is_up_to_date_and_its_staging_is_cleared() {
    let seeded = Seeded::new();
    let staged = seeded.seed("0.2.1", "0.2.2").unwrap();
    let store = seeded.store();
    assert_eq!(
        offline_check(&store, Target::WindowsX86_64, v("0.2.2")),
        UpdateStatus::UpToDate
    );
    assert!(!staged.exists());
    drop(store);
    assert!(seeded.seed("0.2.2", "0.2.2").is_err());
}

#[test]
fn changed_staged_bytes_are_not_reported_ready() {
    let seeded = Seeded::new();
    let staged = seeded.seed("0.2.1", "0.2.2").unwrap();
    fs::write(&staged, b"MZ setup 0.2.2 changed").unwrap();
    assert_eq!(
        offline_check(&seeded.store(), Target::WindowsX86_64, v("0.2.1")),
        UpdateStatus::Failed(Failure::Rejected)
    );
}

#[test]
fn qa_builds_trust_the_public_test_root() {
    assert_eq!(super::super::embedded_root(), Some(test_root()));
}
