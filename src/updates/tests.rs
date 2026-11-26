//! Public test keys and inert bytes only, mirroring the Python release fixture.
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{Value as Json, json};

use super::json::{canonical, parse};
use super::*;

const VECTORS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/updates/client_vectors/"
);

fn vector(name: &str) -> Vec<u8> {
    std::fs::read(format!("{VECTORS}{name}")).unwrap()
}

pub(super) fn canon(value: &Json) -> Vec<u8> {
    canonical(&parse(&serde_json::to_vec(value).unwrap()).unwrap())
}

struct TestKey {
    id: String,
    public: Json,
    pair: Ed25519KeyPair,
}

fn key(label: &str) -> TestKey {
    let seed =
        sha256(format!("PUBLIC RELEASE MANIFEST TEST KEY; NEVER PRODUCTION:{label}").as_bytes());
    let pair = Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
    let public = json!({"keytype": "ed25519", "scheme": "ed25519",
        "keyval": {"public": to_hex(pair.public_key().as_ref())}});
    TestKey {
        id: to_hex(&sha256(&canon(&public))),
        public,
        pair,
    }
}

fn header(schema: &str) -> serde_json::Map<String, Json> {
    json!({"schema": schema, "product": "token-ashtray",
        "repository": "rammicz/token-ashtray", "channel": "stable"})
    .as_object()
    .unwrap()
    .clone()
}

pub(super) const ROOTS: [&str; 3] = ["root-a", "root-b", "root-c"];

pub(super) fn policy(version: u64, release: &[&str], roots: &[&str]) -> Json {
    let ids = |names: &[&str]| names.iter().map(|name| key(name).id).collect::<Vec<_>>();
    let keys: serde_json::Map<String, Json> = roots
        .iter()
        .chain(release)
        .map(|name| {
            let key = key(name);
            (key.id, key.public)
        })
        .collect();
    let mut value = header(ROOT_SCHEMA);
    value.insert("version".into(), json!(version));
    value.insert("keys".into(), keys.into());
    value.insert(
        "roles".into(),
        json!({"root": {"keyids": ids(roots), "threshold": 2},
            "release": {"keyids": ids(release), "threshold": 1}}),
    );
    value.into()
}

pub(super) fn sign(body: &Json, names: &[&str]) -> Json {
    let message = canon(body);
    let signatures: Vec<Json> = names
        .iter()
        .map(|name| {
            let key = key(name);
            json!({"keyid": key.id, "sig": to_hex(key.pair.sign(&message).as_ref())})
        })
        .collect();
    json!({"signed": body, "signatures": signatures})
}

pub(super) fn payload(target: Target, version: &str) -> Vec<u8> {
    format!("SYNTHETIC NONEXECUTABLE {} {version}", target.triple()).into_bytes()
}

pub(super) fn body(version: &str, sequence: u64, epoch: u64) -> Json {
    let rows: serde_json::Map<String, Json> = Target::ALL
        .iter()
        .map(|target| {
            let bytes = payload(*target, version);
            let row = json!({"filename": target.artifact_name(v(version)), "purpose": "installer",
                "artifact_kind": target.artifact_kind(), "length": bytes.len(),
                "sha256": to_hex(&sha256(&bytes)), "app_sha256": "b".repeat(64),
                "descriptor_sha256": "c".repeat(64)});
            (target.triple().to_owned(), row)
        })
        .collect();
    let mut value = header(MANIFEST_SCHEMA);
    for (field, item) in [
        ("version", json!(version)),
        ("tag", json!(format!("v{version}"))),
        ("sequence", json!(sequence)),
        ("root_version", json!(epoch)),
        ("source_sha", json!("a".repeat(40))),
        ("artifacts", rows.into()),
    ] {
        value.insert(field.into(), item);
    }
    value.into()
}

pub(super) fn bundle(body: &Json, names: &[&str], transitions: &[Json]) -> Vec<u8> {
    canon(&json!({"manifest": sign(body, names), "transitions": transitions}))
}

pub(super) fn example(version: &str, sequence: u64) -> Vec<u8> {
    bundle(&body(version, sequence, 1), &["old"], &[])
}

pub(super) fn root_v1() -> RootPolicy {
    RootPolicy::from_json(&canon(&policy(1, &["old"], &ROOTS))).unwrap()
}

pub(super) fn v(text: &str) -> Version {
    Version::parse(text).unwrap()
}

/// Takes the store lock that this process just released. On Unix a process
/// that a parallel test is spawning holds a copy of the lock descriptor until
/// it executes, so the release can show up a moment late.
pub(super) fn reopen(state: &std::path::Path, staging: &std::path::Path) -> Store {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(store) = Store::open(state, staging).unwrap() {
            return store;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the update lock stayed held"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn fresh() -> State {
    State::initial(root_v1())
}

/// Verifies for Linux; `tag` is the observed `vX.Y.Z` discovery tag.
fn check(raw: &[u8], state: &State, installed: &str, tag: &str) -> Verification {
    verify(raw, state, Target::LinuxX86_64, v(installed), observed(tag))
}

fn observed(tag: &str) -> Version {
    v(tag.strip_prefix('v').unwrap())
}

fn accepted(raw: &[u8], state: &State) -> (Status, State) {
    let verification = check(raw, state, "0.2.1", "v0.2.2");
    (verification.result.unwrap().status, verification.state)
}

fn code(verification: Verification) -> &'static str {
    verification.result.unwrap_err().code()
}

fn rejected(raw: &[u8], state: &State) -> &'static str {
    code(check(raw, state, "0.2.1", "v0.2.2"))
}

/// Replaces exactly one occurrence of `from` in the raw bundle bytes.
fn edit(raw: &[u8], from: &str, to: &str) -> Vec<u8> {
    let text = std::str::from_utf8(raw).unwrap();
    assert_eq!(text.matches(from).count(), 1, "{from}");
    text.replacen(from, to, 1).into_bytes()
}

#[test]
fn newer_signed_release_is_pending_for_each_target_then_exact_retry() {
    let raw = example("0.2.2", 2);
    for target in Target::ALL {
        let result = verify(&raw, &fresh(), target, v("0.2.1"), v("0.2.2"));
        let state = result.state;
        assert_eq!(state.root, root_v1());
        let release = ReleaseFloor {
            sequence: 2,
            version: v("0.2.2"),
            digest: sha256(&raw),
            pending: true,
        };
        assert_eq!(state.release, Some(release));
        let first = result.result.unwrap();
        assert_eq!(first.status, Status::Pending);
        assert_eq!(first.artifact.target, target);
        assert_eq!(first.artifact.filename, target.artifact_name(v("0.2.2")));
        let bytes = payload(target, "0.2.2");
        first.artifact.verify_bytes(&bytes).unwrap();
        let mut changed = bytes.clone();
        changed[0] ^= 1;
        for wrong in [changed, [bytes.as_slice(), b"x"].concat(), Vec::new()] {
            assert_eq!(
                first.artifact.verify_bytes(&wrong).unwrap_err().code(),
                "artifact-bytes"
            );
        }
        let retry = verify(&raw, &state, target, v("0.2.1"), v("0.2.2"));
        assert_eq!(retry.result.unwrap().status, Status::Retry);
        assert_eq!(retry.state, state);
    }
}

#[test]
fn flipped_signed_byte_fails_the_release_signature() {
    let raw = edit(
        &example("0.2.2", 2),
        "\"version\":\"0.2.2\"",
        "\"version\":\"0.2.3\"",
    );
    assert_eq!(
        code(check(&raw, &fresh(), "0.2.1", "v0.2.3")),
        "signature-release"
    );
}

#[test]
fn duplicate_object_keys_are_rejected_even_with_equal_values() {
    let raw = example("0.2.2", 2);
    let doubled = edit(
        &raw,
        "\"version\":\"0.2.2\"",
        "\"version\":\"0.2.2\",\"version\":\"0.2.2\"",
    );
    assert_eq!(rejected(&doubled, &fresh()), "duplicate-json-key");
    assert_eq!(
        rejected(br#"{"manifest":{},"manifest":{}}"#, &fresh()),
        "duplicate-json-key"
    );
}

#[test]
fn non_integer_numbers_are_rejected() {
    let raw = example("0.2.2", 2);
    for number in ["2.0", "2e0", "2E0", "02", "-2", "9007199254740992"] {
        let changed = edit(&raw, "\"sequence\":2", &format!("\"sequence\":{number}"));
        assert_eq!(rejected(&changed, &fresh()), "json-number", "{number}");
    }
}

#[test]
fn strings_outside_printable_ascii_are_rejected() {
    for raw in [
        &b"\"a\x7fb\""[..],
        br#""a\u007fb""#,
        br#""a\u001fb""#,
        br#""a\nb""#,
        b"\"a\tb\"",
        "\"caf\u{e9}\"".as_bytes(),
        br#""caf\u00e9""#,
    ] {
        assert_eq!(parse(raw).unwrap_err().code(), "json-string", "{raw:?}");
    }
    let raw = edit(
        &example("0.2.2", 2),
        "\"tag\":\"v0.2.2\"",
        "\"tag\":\"v0.2.2\\u007f\"",
    );
    assert_eq!(rejected(&raw, &fresh()), "json-string");
}

#[test]
fn other_malformed_json_is_rejected() {
    for raw in [
        &b"NaN"[..],
        b"null",
        b"{\"a\":null}",
        b"[1,]",
        b"{\"a\":1,}",
        b"{\"a\" 1}",
        b"[1] [2]",
        b"\"open",
        b"tru",
        b"[\"\\x41\"]",
        b"[\"\\u00G1\"]",
    ] {
        assert!(parse(raw).is_err(), "{}", String::from_utf8_lossy(raw));
    }
    assert_eq!(parse(&[b'['; 40]).unwrap_err().code(), "json-depth");
    assert!(parse(b" { \"a\" : [ true , false , 0 ] } \n").is_ok());
}

#[test]
fn canonical_bytes_match_the_python_fixture_serializer() {
    let expected = vector("canonical-expected.json");
    assert_eq!(
        canonical(&parse(&vector("canonical-input.json")).unwrap()),
        expected
    );
    assert_eq!(canonical(&parse(&expected).unwrap()), expected);
}

#[test]
fn python_signed_vectors_match_ring_and_verify() {
    let root = vector("root-v1.json");
    let raw = vector("bundle-0.2.2.json");
    assert_eq!(canon(&policy(1, &["old"], &ROOTS)), root);
    assert_eq!(example("0.2.2", 2), raw);
    let policy = RootPolicy::from_json(&root).unwrap();
    assert_eq!(policy.canonical_json(), root.as_slice());
    let result = check(&raw, &State::initial(policy), "0.2.1", "v0.2.2")
        .result
        .unwrap();
    assert_eq!(result.status, Status::Pending);
    result
        .artifact
        .verify_bytes(&payload(Target::LinuxX86_64, "0.2.2"))
        .unwrap();
}

#[test]
fn signed_identity_and_row_errors_are_rejected() {
    let fresh = fresh();
    let resign = |edit: &dyn Fn(&mut serde_json::Map<String, Json>)| {
        let mut value = body("0.2.2", 2, 1);
        edit(value.as_object_mut().unwrap());
        bundle(&value, &["old"], &[])
    };
    for (expected, field, value) in [
        ("version-tag", "version", json!("0.2.3")),
        ("stable-version", "version", json!("0.2.2-beta")),
        ("stable-version", "version", json!("0.02.2")),
        ("version-tag", "tag", json!("v0.2.3")),
        ("schema-product-channel", "channel", json!("beta")),
        (
            "schema-product-channel",
            "schema",
            json!("release-manifest-fixture-v1"),
        ),
        ("root-binding", "root_version", json!(2)),
        ("root-binding", "root_version", json!(true)),
        ("source-binding", "source_sha", json!("A".repeat(40))),
        ("release-sequence", "sequence", json!(0)),
        ("schema-fields", "fixture_only", json!(true)),
    ] {
        let raw = resign(&|b| {
            b.insert(field.into(), value.clone());
        });
        assert_eq!(rejected(&raw, &fresh), expected, "{field} {value}");
    }
    let missing = resign(&|b| {
        b.remove("source_sha");
    });
    let foreign = resign(&|b| {
        let rows = b["artifacts"].as_object_mut().unwrap();
        let row = rows.remove("x86_64-unknown-linux-gnu").unwrap();
        rows.insert("aarch64-unknown-linux-gnu".into(), row);
    });
    for raw in [missing, foreign] {
        assert_eq!(rejected(&raw, &fresh), "schema-fields");
    }
    for (field, value) in [
        ("filename", json!("../evil")),
        ("artifact_kind", json!("update-zip")),
        ("purpose", json!("update")),
        ("length", json!(MAX_ARTIFACT + 1)),
        ("sha256", json!("B".repeat(64))),
    ] {
        for target in Target::ALL {
            let raw = resign(&|b| {
                b["artifacts"][target.triple()][field] = value.clone();
            });
            assert_eq!(
                rejected(&raw, &fresh),
                "artifact-binding",
                "{field} {target:?}"
            );
        }
    }
}

#[test]
fn same_sequence_or_version_cannot_change_bytes() {
    let (_, state) = accepted(&example("0.2.2", 2), &fresh());
    for (version, sequence, expected) in [
        ("0.2.3", 1, "release-rollback"),
        ("0.2.3", 2, "same-sequence-change"),
        ("0.2.2", 3, "same-version-change"),
        ("0.2.1", 3, "release-rollback"),
    ] {
        let raw = example(version, sequence);
        let result = check(&raw, &state, "0.2.1", &format!("v{version}"));
        assert_eq!(result.state, state);
        assert_eq!(code(result), expected, "{version} {sequence}");
    }
    let mut padded = example("0.2.2", 2);
    padded.push(b'\n');
    assert_eq!(rejected(&padded, &state), "same-sequence-change");
}

#[test]
fn installed_floor_and_already_installed_release() {
    let raw = example("0.2.2", 2);
    assert_eq!(
        code(check(&raw, &fresh(), "0.2.3", "v0.2.2")),
        "installed-downgrade"
    );
    let first = check(&raw, &fresh(), "0.2.2", "v0.2.2");
    assert!(!first.state.release.unwrap().pending);
    assert_eq!(first.result.unwrap().status, Status::NoUpdate);
    let again = check(&raw, &first.state, "0.2.2", "v0.2.2");
    assert_eq!(again.result.unwrap().status, Status::NoUpdate);
}

#[test]
fn dual_threshold_rotation_advances_and_persists_the_root_floor() {
    let second = policy(2, &["new"], &ROOTS);
    let third = policy(3, &["third"], &["root-d", "root-e", "root-f"]);
    let chain = [
        sign(&second, &["root-a", "root-b"]),
        sign(&third, &["root-a", "root-b", "root-d", "root-e"]),
    ];
    let raw = bundle(&body("0.2.2", 2, 3), &["third"], &chain);
    let (status, state) = accepted(&raw, &fresh());
    assert_eq!(status, Status::Pending);
    assert_eq!(state.root.version(), 3);
    let restored = RootPolicy::from_json(state.root.canonical_json()).unwrap();
    assert_eq!(restored, state.root);
    // The old release key is no longer trusted after the advance.
    let old = example("0.2.3", 3);
    let refused = check(&old, &state, "0.2.1", "v0.2.3");
    assert_eq!(refused.state, state);
    assert_eq!(code(refused), "release-key-authority");
    assert_eq!(accepted(&raw, &state).0, Status::Retry);
}

#[test]
fn authenticated_root_advance_is_reported_when_the_release_fails() {
    let second = policy(2, &["new"], &ROOTS);
    let chain = [sign(&second, &["root-a", "root-b"])];
    // Still signed by the retired release key.
    let raw = bundle(&body("0.2.2", 2, 2), &["old"], &chain);
    let result = check(&raw, &fresh(), "0.2.1", "v0.2.2");
    assert_eq!(result.state.root.version(), 2);
    assert_eq!(
        result.state.root.canonical_json(),
        canon(&second).as_slice()
    );
    assert_eq!(result.state.release, None);
    assert_eq!(code(result), "release-key-authority");
    // A wrong root binding after a valid advance also keeps the advance.
    let raw = bundle(&body("0.2.2", 2, 1), &["new"], &chain);
    let result = check(&raw, &fresh(), "0.2.1", "v0.2.2");
    assert_eq!(result.state.root.version(), 2);
    assert_eq!(code(result), "root-binding");
}

#[test]
fn invalid_rotations_are_rejected_without_an_advance() {
    let second = policy(2, &["new"], &["root-d", "root-e", "root-f"]);
    let valid = sign(&second, &["root-a", "root-b", "root-d", "root-e"]);
    let mut duplicated = sign(&second, &["root-a", "root-d", "root-e"]);
    let first = duplicated["signatures"][0].clone();
    duplicated["signatures"].as_array_mut().unwrap().push(first);
    let changed_v1 = policy(1, &["new"], &ROOTS);
    let cases = [
        (
            "root-sequence",
            vec![sign(&policy(3, &["new"], &ROOTS), &["root-a", "root-b"])],
        ),
        ("signature-root", vec![sign(&second, &["root-a", "root-b"])]),
        ("signature-root", vec![sign(&second, &["root-d", "root-e"])]),
        ("signature-shape", vec![duplicated]),
        ("signature-root", vec![sign(&second, &["old", "new"])]),
        ("rotation-bound", vec![valid.clone(); MAX_TRANSITIONS + 1]),
        ("root-order", vec![valid.clone(), valid.clone()]),
        (
            "root-equivocation",
            vec![sign(&changed_v1, &["root-a", "root-b"])],
        ),
    ];
    for (expected, chain) in cases {
        let raw = bundle(&body("0.2.2", 2, 2), &["new"], &chain);
        let result = check(&raw, &fresh(), "0.2.1", "v0.2.2");
        assert_eq!(result.state, fresh(), "{expected}");
        assert_eq!(code(result), expected);
    }
    // A retained record equal to the trusted root is harmless.
    let same = sign(&policy(1, &["old"], &ROOTS), &["root-a", "root-b"]);
    let raw = bundle(&body("0.2.2", 2, 1), &["old"], &[same]);
    assert_eq!(accepted(&raw, &fresh()).0, Status::Pending);
}

#[test]
fn root_keys_cannot_sign_a_release() {
    let raw = bundle(&body("0.2.2", 2, 1), &["root-a", "root-b"], &[]);
    assert_eq!(rejected(&raw, &fresh()), "release-key-authority");
}

#[test]
fn root_policy_shape_is_enforced() {
    let mutate = |edit: &dyn Fn(&mut Json)| {
        let mut value = policy(1, &["old"], &ROOTS);
        edit(&mut value);
        RootPolicy::from_json(&canon(&value))
            .map(|_| ())
            .unwrap_err()
            .code()
    };
    let old = key("old").id;
    let root_a = key("root-a").id;
    assert_eq!(
        mutate(&|p| p["roles"]["root"]["threshold"] = json!(1)),
        "root-threshold"
    );
    assert_eq!(
        mutate(&|p| p["roles"]["release"]["threshold"] = json!(2)),
        "role-policy"
    );
    assert_eq!(
        mutate(&|p| p["roles"]["release"]["keyids"] = json!([old, root_a])),
        "role-policy"
    );
    assert_eq!(
        mutate(&|p| p["roles"]["release"]["keyids"] = json!([old, old])),
        "role-policy"
    );
    assert_eq!(mutate(&|p| p["version"] = json!(0)), "root-version");
    assert_eq!(
        mutate(&|p| p["keys"][&old]["keyval"]["public"] = json!("0".repeat(64))),
        "root-key"
    );
    assert_eq!(
        mutate(&|p| {
            let extra = key("new");
            p["keys"][&extra.id] = extra.public;
        }),
        "unused-policy-key"
    );
}

#[test]
fn metadata_bounds_are_enforced() {
    assert_eq!(rejected(b"", &fresh()), "metadata-bound");
    assert_eq!(
        rejected(&vec![b' '; MAX_METADATA + 1], &fresh()),
        "metadata-bound"
    );
}

#[test]
fn stable_versions_are_strict() {
    for text in ["0.2.1", "10.0.0", "0.0.0"] {
        assert_eq!(v(text).to_string(), text);
    }
    for text in [
        "0.2",
        "0.2.1.0",
        "01.2.3",
        "0.2.1-rc1",
        "0.2.1+b",
        "v0.2.1",
        "0..1",
        "99999999999999999999.0.0",
    ] {
        assert!(Version::parse(text).is_err(), "{text}");
    }
    assert!(v("0.10.0") > v("0.9.9"));
}

#[test]
fn release_once_known_as_installed_is_not_pending_after_a_downgrade() {
    // Accepted as no-update while 0.2.2 was installed; 0.2.1 runs again later.
    let raw = example("0.2.2", 2);
    let known = check(&raw, &fresh(), "0.2.2", "v0.2.2").state;
    assert!(!known.release.unwrap().pending);
    let result = check(&raw, &known, "0.2.1", "v0.2.2");
    assert_eq!(result.state, known);
    assert_eq!(code(result), "not-pending");
}
