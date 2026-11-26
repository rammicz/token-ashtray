# Owner-held release signing — event-driven candidate

## Mandate and correction

The owner holds production signing keys and approves new releases. The owner's
2026-11-22 clarification rejects recurring operator maintenance entirely. The
former weekly four-role proposal is superseded; its fixtures remain historical.
This active procedure uses immutable owner-signed release manifests, with no
calendar renewal, expiring wrapper or online signing service. It preserves G7,
paused Actions and the [canonical release contract](public-channel-contract.md).

Owner approval of a new release remains necessary. Routine discovery, validation,
download and safe Quit/apply are intended to run automatically after client
opt-in, not require owner attendance for each installation. Real signed, public
and native acceptance of the updater remains pending. No production key has been
created and no signing ceremony has been performed.

## Separate root and release authority

Candidate bootstrap: an owner-verified pinned Ed25519 root-policy blob with a
2-of-3 offline root threshold and one distinct 1-of-1 release key. One human
custodian controls the keys; independent storage is not independent human approval.
Root keys authorize only bounded sequential policy changes, with both old and
new root thresholds over the same N+1 policy. The ordinary release key authorizes
only immutable release manifests. Its compromise cannot install a replacement
trusted key set; it can authorize malicious releases until a genuine root change
or manual recovery is accepted. Root-threshold compromise still requires a
separately verified bootstrap recovery, never network trust-on-first-use.

Generate/hold production material only in the owner's separate protected signing
environment. Keep root key pairs and encrypted backups independently protected,
with recovery secrets and sensitive locations outside GitHub/session. Preserve
public bootstrap fingerprints, root transitions and an owner-held monotonic
release/root ledger with backups. No periodic owner review is required by this
profile: verify custody/recovery at initial setup or an actual key/loss/compromise
change. Private keys are never uploaded to GitHub, Actions, developer fixtures,
requests, issues, logs or this session. No agent fallback custodian is introduced.

At a rotation/revocation event, obtain distinct old/new root authorizations,
verify stale-client catch-up within the contract bounds and retain all intermediate
records. The next approved release carries the relevant transition bundle and
new release signature. If a client cannot bridge the bounded chain, use explicit
verified manual recovery. Lost release keys can be replaced through root; a lost
root threshold cannot be repaired by accepting a newly downloaded public key.
There is no time expiry to renew between these events.

## Freeze an unsigned release request

Prepare the exact two platform installers and reviewed build receipts at a clean
source SHA. The [unsigned inspector](public-channel-contract.md#offline-evidence-and-unsigned-inventory)
checks package/resource/app/metrics byte consistency. Its source/version/compiler
fields remain assertions until independent build receipts establish them.

The frozen unsigned request contains profile ID/hash, unique request ID, intended
stable version/tag, release authorization sequence, trusted root-policy hash/
version, exact artifact names/kinds/targets/purpose, size/SHA-256, app/resource
identities, actual build/native/legal verification receipts and the complete
canonical manifest `signed` payload/hash. Include proposed policy transitions
only for a key event, each with its exact canonical payload/hash and continuity
plan. Record the request's exact external SHA-256 without self-referential fields.
Transport its nonsecret bytes and digest to the owner through independently
checked channels; an adjacent untrusted checksum is not authentication.

The owner compares intent, artifacts and source/build receipts with the local
trusted ledger before approving. Reject unknown/missing proof, changed source/
version/resource identity or reused sequence/tag with different bytes. A signature
must not promote an arbitrary caller source/compiler assertion into a verified
build fact. Changes require a new request, and changed published app artifacts
require a new stable release version.

## Sign, verify and publish once per approved release

On approval, the owner signs the frozen manifest payload with the separate
release key. Sign root transition payloads with both root thresholds only when
needed. Return signed envelopes plus an owner-kept local response receipt binding
request/profile digests, full canonical payload hashes, exact final envelope/bundle
hashes, role key IDs, root version, release sequence/version and artifact/build
receipt identities. The request/response records stay outside the public feed;
no extra client approval extension is introduced. A refusal authorizes no release.

An independent verifier starts from pinned trusted root, checks each transition,
release signature, requested payload hash, exact source/version/tag/target/artifact
bindings and local receipt. An unsigned response index alone is not authority.
Freeze final bytes after all signatures; do not pretty-print after recording the
bundle digest. There are no snapshot/timestamp parent projections in this profile.
Existing separately owned signing-request experiments keep their historical scope.

Upload the manifest as `token-ashtray-updates.json` with both approved versioned
installers in the same draft. Verify the assets and set the intended latest stable
release when the owner publishes. GitHub [documents the fixed latest-asset suffix](https://docs.github.com/en/repositories/releasing-projects-on-github/linking-to-releases#linking-to-the-latest-release).
The manifest is immutable within each versioned release; only GitHub's untrusted
latest designation changes on the next publication. No mutable control release,
asset replacement or idle-month metadata job is needed. Actions remain disabled;
source/tag pushes alone do not deliver packages. No upload/publication/settings
action is performed by this document.

## Client state and admitted freeze risk

The contract requires trusted root and maximum observed release sequence/version/
exact bundle digest outside disposable cache. Persist authenticated root changes
before authorizing releases. Accept unchanged pending bytes for retry; reject
changed bytes at the same sequence, stale trust, or installed-version downgrade.
Owner-approved key changes and release publication are event-driven; client
checking/downloading does not need a new owner signature.

Discovery can hide a never-seen newer release or replay a valid old upgrade. This
custom profile has **no cryptographic time-based freshness** and permits an
unbounded freeze. No signed date or latest URL proves currentness; local floors
protect only already-observed history. The owner must review this security
tradeoff before accepting the exact production profile. Routine automatic
operation is settled; cryptographic freshness is not silently claimed.

## Required verification before adoption

- Pin the real schema/serialization/implementation and public bootstrap; verify
  separate key custody, backup/recovery and root/release authority separation.
- Compare the frozen request, signed response, real build receipts and both actual
  installer/resource identities; prove mutation/wrong source/version/tag/target
  rejection and monotonic state through cancellation/restart/crash.
- Exercise dual-threshold sequential rotation, revoked/unknown/duplicate keys,
  bounded stale-client recovery, exact pending retry and installed downgrade refusal.
- Verify anonymous project latest→same-repo tag→direct/exact-CDN discovery and both
  real artifact downloads, statuses, signatures/hashes and tamper rejection. Never
  widen paths/hosts or add credentials to accommodate a mismatch.
- Demonstrate interrupted publication/unavailable discovery leaves installed app
  and trust state safe; explicitly show old-valid-manifest withholding is possible.
- Implement and natively verify opt-in automatic checks/download and safe Quit/apply
  on Linux and Windows; current fixtures do not deliver an updater or satisfy public acceptance.

The [new synthetic fixture](../../tests/fixtures/updates/release_manifest/README.md)
checks selected offline rules with public test keys and inert bytes. Historical
four-role fixtures and local request experiments are unchanged and not a second
active deployment plan. No production key, live signed asset or public proof exists
merely because these checks pass.
