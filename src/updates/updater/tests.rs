//! The whole check against the local HTTP fixture and a temporary store.
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use tempfile::TempDir;

use super::*;
use crate::updates::ReleaseFloor;
use crate::updates::store::STATE_FILE;
use crate::updates::tests::{
    ROOTS, body, bundle, canon, example, payload, policy, reopen, root_v1, sign, v,
};
use crate::updates::transport::tests::{
    ARTIFACT, LATEST, Server, TAGGED, delayed, ok, path, redirect,
};

struct Dirs {
    _temporary: TempDir,
    state: PathBuf,
    staging: PathBuf,
}

fn dirs() -> Dirs {
    let temporary = tempfile::tempdir().unwrap();
    Dirs {
        state: temporary.path().join("data"),
        staging: temporary.path().join("cache"),
        _temporary: temporary,
    }
}

impl Dirs {
    fn open(&self) -> Store {
        reopen(&self.state, &self.staging)
    }
}

fn artifact() -> Vec<u8> {
    payload(Target::LinuxX86_64, "0.2.2")
}

/// Serves `raw` as the latest manifest and the 0.2.2 Linux artifact.
fn feed(raw: &[u8]) -> Server {
    feed_with(raw, &artifact())
}

fn feed_with(raw: &[u8], artifact: &[u8]) -> Server {
    Server::new(&[
        (LATEST, vec![redirect(TAGGED)]),
        (TAGGED, vec![ok(raw)]),
        (ARTIFACT, vec![ok(artifact)]),
    ])
}

async fn run(store: &Store, server: &Server, installed: &str) -> Result<Outcome, CheckError> {
    let transport = server.transport(Duration::from_secs(5));
    check_once(
        store,
        &transport,
        &root_v1(),
        Target::LinuxX86_64,
        v(installed),
    )
    .await
}

fn downloads(server: &Server) -> usize {
    server
        .paths()
        .iter()
        .filter(|request| **request == path(ARTIFACT))
        .count()
}

#[tokio::test]
async fn pending_release_is_persisted_staged_and_retried_without_download() {
    let dirs = dirs();
    let server = feed(&example("0.2.2", 2));
    let store = dirs.open();
    let Outcome::Staged { path, record } = run(&store, &server, "0.2.1").await.unwrap() else {
        panic!("expected a staged artifact");
    };
    assert_eq!(fs::read(&path).unwrap(), artifact());
    assert_eq!(store.verified(&record), Some(path.clone()));
    let saved = store.load().unwrap().unwrap().release.unwrap();
    assert_eq!(
        (saved.sequence, saved.version, saved.pending),
        (2, v("0.2.2"), true)
    );
    drop(store);

    // A later run sees the same bundle, keeps the verified file and does not
    // download it again.
    let store = dirs.open();
    let again = run(&store, &server, "0.2.1").await.unwrap();
    assert_eq!(again, Outcome::Staged { path, record });
    assert_eq!(downloads(&server), 1);
}

#[tokio::test]
async fn newer_embedded_root_replaces_an_older_stored_root() {
    let dirs = dirs();
    let store = dirs.open();
    store.save(&State::initial(root_v1())).unwrap();
    // A later build embeds root version 2, which retired the "old" release key.
    let second = RootPolicy::from_json(&canon(&policy(2, &["new"], &ROOTS))).unwrap();
    let server = feed(&bundle(&body("0.2.2", 2, 2), &["new"], &[]));
    let transport = server.transport(Duration::from_secs(5));
    let outcome = check_once(&store, &transport, &second, Target::LinuxX86_64, v("0.2.1")).await;
    assert!(matches!(outcome, Ok(Outcome::Staged { .. })), "{outcome:?}");
    assert_eq!(store.load().unwrap().unwrap().root.version(), 2);

    // An older embedded root never replaces a newer stored one.
    let server = feed(&bundle(&body("0.2.2", 2, 2), &["new"], &[]));
    let transport = server.transport(Duration::from_secs(5));
    let outcome = check_once(
        &store,
        &transport,
        &root_v1(),
        Target::LinuxX86_64,
        v("0.2.1"),
    )
    .await;
    assert!(matches!(outcome, Ok(Outcome::Staged { .. })), "{outcome:?}");
    assert_eq!(store.load().unwrap().unwrap().root.version(), 2);
}

#[tokio::test]
async fn concurrent_checks_on_one_store_are_serialized() {
    let dirs = dirs();
    let server = Server::new(&[
        (LATEST, vec![redirect(TAGGED)]),
        (TAGGED, vec![ok(&example("0.2.2", 2))]),
        (
            ARTIFACT,
            vec![delayed(&artifact(), Duration::from_millis(300))],
        ),
    ]);
    let store = dirs.open();
    let (first, second) =
        tokio::join!(run(&store, &server, "0.2.1"), run(&store, &server, "0.2.1"));
    let first = first.unwrap();
    assert_eq!(second.unwrap(), first);
    assert_eq!(downloads(&server), 1);
    let Outcome::Staged { path, record } = first else {
        panic!("expected a staged artifact");
    };
    assert_eq!(store.verified(&record), Some(path));
}

#[tokio::test]
async fn authenticated_root_advance_is_persisted_when_the_release_is_rejected() {
    let dirs = dirs();
    let second = policy(2, &["new"], &ROOTS);
    let chain = [sign(&second, &["root-a", "root-b"])];
    // The manifest is still signed by the release key the rotation retired.
    let server = feed(&bundle(&body("0.2.2", 2, 2), &["old"], &chain));
    let store = dirs.open();
    let result = run(&store, &server, "0.2.1").await;
    assert!(
        matches!(result, Err(CheckError::Rejected(reject)) if reject.code() == "release-key-authority"),
        "{result:?}"
    );
    let saved = store.load().unwrap().expect("the root advance was saved");
    assert_eq!(saved.root.version(), 2);
    assert_eq!(saved.release, None);
    assert_eq!(downloads(&server), 0);
}

#[tokio::test]
async fn tampered_staged_file_is_downloaded_again() {
    let dirs = dirs();
    let server = feed(&example("0.2.2", 2));
    let store = dirs.open();
    let Outcome::Staged { path, .. } = run(&store, &server, "0.2.1").await.unwrap() else {
        panic!("expected a staged artifact");
    };
    fs::write(&path, b"replaced").unwrap();
    run(&store, &server, "0.2.1").await.unwrap();
    assert_eq!(fs::read(&path).unwrap(), artifact());
    assert_eq!(downloads(&server), 2);
}

#[tokio::test]
async fn already_installed_release_clears_staging() {
    let dirs = dirs();
    let server = feed(&example("0.2.2", 2));
    let store = dirs.open();
    fs::write(dirs.staging.join("leftover.partial"), b"old").unwrap();
    fs::create_dir(dirs.staging.join("leftover-dir")).unwrap();
    assert_eq!(
        run(&store, &server, "0.2.2").await.unwrap(),
        Outcome::NoUpdate
    );
    assert_eq!(fs::read_dir(&dirs.staging).unwrap().count(), 0);
    assert!(!store.load().unwrap().unwrap().release.unwrap().pending);
    assert_eq!(downloads(&server), 0);
}

#[tokio::test]
async fn bad_artifact_bytes_leave_nothing_staged() {
    let dirs = dirs();
    let server = feed_with(&example("0.2.2", 2), b"not the signed artifact");
    let store = dirs.open();
    let result = run(&store, &server, "0.2.1").await;
    assert!(
        matches!(result, Err(CheckError::Fetch(FetchError::Rejected(reject))) if reject.code() == "artifact-bytes"),
        "{result:?}"
    );
    assert_eq!(fs::read_dir(&dirs.staging).unwrap().count(), 0);
    // The floors were still recorded, so the next run retries this release.
    assert!(store.load().unwrap().unwrap().release.unwrap().pending);
}

#[tokio::test]
async fn failed_discovery_leaves_the_state_untouched() {
    let dirs = dirs();
    let store = dirs.open();
    let server = Server::new(&[]);
    let result = run(&store, &server, "0.2.1").await;
    assert!(
        matches!(result, Err(CheckError::Fetch(FetchError::Status(404)))),
        "{result:?}"
    );
    assert!(store.load().unwrap().is_none());
}

#[tokio::test]
async fn corrupt_state_is_reported_and_never_reset() {
    let dirs = dirs();
    let store = dirs.open();
    let state_file = dirs.state.join(STATE_FILE);
    fs::write(&state_file, b"{\"format\":1}").unwrap();
    let server = feed(&example("0.2.2", 2));
    let result = run(&store, &server, "0.2.1").await;
    assert!(
        matches!(result, Err(CheckError::Store(StoreError::Corrupt))),
        "{result:?}"
    );
    assert!(server.paths().is_empty());
    assert_eq!(fs::read(&state_file).unwrap(), b"{\"format\":1}");
}

fn floor(sequence: u64) -> ReleaseFloor {
    ReleaseFloor {
        sequence,
        version: v("1.2.3"),
        digest: [0xab; 32],
        pending: true,
    }
}

#[test]
fn store_round_trips_state_and_holds_an_exclusive_lock() {
    let dirs = dirs();
    let store = dirs.open();
    assert!(Store::open(&dirs.state, &dirs.staging).unwrap().is_none());
    let mut state = State::initial(root_v1());
    store.save(&state).unwrap();
    assert_eq!(store.load().unwrap(), Some(state.clone()));
    state.release = Some(floor(7));
    store.save(&state).unwrap();
    assert_eq!(store.load().unwrap(), Some(state));
    let entries: Vec<_> = fs::read_dir(&dirs.state)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(
        entries.len(),
        2,
        "only the state and lock files: {entries:?}"
    );
    drop(store);
    // Dropping the store releases its lock.
    dirs.open();

    let text = fs::read_to_string(dirs.state.join(STATE_FILE)).unwrap();
    for corrupt in [
        text.replace("\"pending\":true", "\"pending\":true,\"extra\":1"),
        text.replace("\"format\":1", "\"format\":2"),
        text.replace("1.2.3", "1.2"),
        text.replace(&"ab".repeat(32), "zz"),
        text.replace("token-ashtray-release-root-v1", "other"),
    ] {
        fs::write(dirs.state.join(STATE_FILE), &corrupt).unwrap();
        let store = dirs.open();
        assert!(matches!(store.load(), Err(StoreError::Corrupt)));
        // A corrupt file is never overwritten either.
        assert!(store.save(&State::initial(root_v1())).is_err());
        assert_eq!(
            fs::read_to_string(dirs.state.join(STATE_FILE)).unwrap(),
            corrupt
        );
    }
}

#[test]
fn store_never_lowers_the_floors() {
    let dirs = dirs();
    let store = dirs.open();
    let second = RootPolicy::from_json(&canon(&policy(2, &["new"], &ROOTS))).unwrap();
    let stored = State {
        root: second,
        release: Some(floor(7)),
    };
    store.save(&stored).unwrap();
    let lower = [
        State {
            release: Some(floor(6)),
            ..stored.clone()
        },
        State {
            release: None,
            ..stored.clone()
        },
        State {
            root: root_v1(),
            ..stored.clone()
        },
    ];
    for state in lower {
        assert!(matches!(store.save(&state), Err(StoreError::Regression)));
    }
    assert_eq!(store.load().unwrap(), Some(stored));
}
