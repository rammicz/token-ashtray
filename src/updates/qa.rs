//! QA ONLY (#52), behind the `qa-offline-update` feature, which no release
//! build enables. It trusts the public TEST release root of the client
//! vectors, never makes a request, and stages a release signed with the
//! public TEST release key from local files. Everything after staging is the
//! production code: the offline re-verification at Quit, the planner, the
//! platform installer, the journal and the setup's own wait and checks.
use std::fs;
use std::path::{Path, PathBuf};

use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{Value as Json, json};

use super::service::{Failure, UpdateStatus};
use super::{
    MANIFEST_SCHEMA, RootPolicy, State, Status, Store, Target, Version, json, sha256, to_hex,
    verify,
};

/// The public test root of `tests/fixtures/updates/client_vectors`.
const TEST_ROOT: &[u8] = include_bytes!("../../tests/fixtures/updates/client_vectors/root-v1.json");
/// The label of the public test release key that root authorizes.
const TEST_RELEASE_KEY: &str = "PUBLIC RELEASE MANIFEST TEST KEY; NEVER PRODUCTION:old";

pub(super) fn test_root() -> RootPolicy {
    RootPolicy::from_json(TEST_ROOT).expect("the client vector root is valid")
}

fn canonical(value: &Json) -> Vec<u8> {
    let raw = serde_json::to_vec(value).expect("JSON values serialize");
    json::canonical(&json::parse(&raw).expect("serde_json writes strict JSON"))
}

/// A bundle for `version` whose row for `target` names `artifact`, whose app
/// is `app`, signed with the public test release key. The other row holds
/// inert synthetic bytes. The sequence follows the version, so a newer
/// version always has a higher sequence and an identical seed is a retry.
fn bundle(target: Target, version: Version, artifact: &[u8], app: &[u8]) -> Vec<u8> {
    let rows: serde_json::Map<String, Json> = Target::ALL
        .iter()
        .map(|row| {
            let synthetic = format!("SYNTHETIC NONEXECUTABLE {} {version}", row.triple());
            let (bytes, app) = if *row == target {
                (artifact, sha256(app))
            } else {
                (synthetic.as_bytes(), [0xbb; 32])
            };
            let value = json!({"filename": row.artifact_name(version), "purpose": "installer",
                "artifact_kind": row.artifact_kind(), "length": bytes.len(),
                "sha256": to_hex(&sha256(bytes)), "app_sha256": to_hex(&app),
                "descriptor_sha256": "c".repeat(64)});
            (row.triple().to_owned(), value)
        })
        .collect();
    let body = json!({"schema": MANIFEST_SCHEMA, "product": "token-ashtray",
        "repository": "rammicz/token-ashtray", "channel": "stable",
        "version": version.to_string(), "tag": format!("v{version}"),
        "sequence": version.major * 1_000_000 + version.minor * 1_000 + version.patch + 1, "root_version": 1,
        "source_sha": "a".repeat(40), "artifacts": rows});
    let key = Ed25519KeyPair::from_seed_unchecked(&sha256(TEST_RELEASE_KEY.as_bytes()))
        .expect("a 32-byte seed");
    let public = json!({"keytype": "ed25519", "scheme": "ed25519",
        "keyval": {"public": to_hex(key.public_key().as_ref())}});
    let signature = json!({"keyid": to_hex(&sha256(&canonical(&public))),
        "sig": to_hex(key.sign(&canonical(&body)).as_ref())});
    canonical(
        &json!({"manifest": {"signed": body, "signatures": [signature]},
        "transitions": []}),
    )
}

/// Stages `setup` as release `version` in `store`, as a successful check
/// would: the production verifier accepts the signed bundle against the
/// stored floors, then the floors, the bundle and the staged file are saved.
/// Returns the staged path.
fn seed_into(
    store: &Store,
    target: Target,
    installed: Version,
    version: Version,
    setup: &Path,
    app: &Path,
) -> Result<PathBuf, String> {
    let artifact = fs::read(setup).map_err(|error| format!("read {}: {error}", setup.display()))?;
    let app = fs::read(app).map_err(|error| format!("read {}: {error}", app.display()))?;
    let raw = bundle(target, version, &artifact, &app);
    let state = store
        .load()
        .map_err(|error| format!("load the update state: {error:?}"))?
        .unwrap_or_else(|| State::initial(test_root()));
    let verification = verify(&raw, &state, target, installed, version);
    let accepted = verification
        .result
        .map_err(|reject| format!("the seeded release was rejected: {reject}"))?;
    if accepted.status == Status::NoUpdate {
        return Err(format!(
            "{version} is not newer than the installed {installed}"
        ));
    }
    store
        .save(&verification.state)
        .map_err(|error| format!("save the update state: {error:?}"))?;
    store
        .save_bundle(&raw)
        .map_err(|error| format!("save the bundle: {error}"))?;
    let record = accepted.artifact;
    store
        .clean_staging(Some(&record.filename))
        .map_err(|error| format!("clean staging: {error}"))?;
    let staging = store.stage(&record);
    fs::write(staging.partial(), &artifact).map_err(|error| format!("stage: {error}"))?;
    staging
        .commit()
        .map_err(|error| format!("stage: {error}"))?;
    store
        .verified(&record)
        .ok_or_else(|| "the staged setup does not match the signed row".to_owned())
}

/// Stages `setup` in the app's own update folders.
pub fn seed(
    installed: Version,
    version: Version,
    setup: &Path,
    app: &Path,
) -> Result<PathBuf, String> {
    let target = Target::current().ok_or("this platform has no channel artifact")?;
    let (state, staging) = Store::default_dirs().ok_or("no per-user update folder")?;
    let store = Store::open(&state, &staging)
        .map_err(|error| format!("open the update state: {error}"))?
        .ok_or("another Token Ashtray process holds the update state; quit it first")?;
    seed_into(&store, target, installed, version, setup, app)
}

/// The check of a QA build: no request, only what [`seed`] staged.
pub(super) fn offline_check(store: &Store, target: Target, installed: Version) -> UpdateStatus {
    match store.pending(target, installed) {
        Some(record) if store.verified(&record).is_some() => {
            UpdateStatus::Ready(record.version.to_string())
        }
        // Production would download the release again.
        Some(_) => UpdateStatus::Failed(Failure::Rejected),
        None => match store.clean_staging(None) {
            Ok(()) => UpdateStatus::UpToDate,
            Err(_) => UpdateStatus::Failed(Failure::Storage),
        },
    }
}

#[cfg(test)]
mod tests;
