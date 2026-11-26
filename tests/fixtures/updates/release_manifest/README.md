# Release-manifest offline fixture — PUBLIC TEST KEYS ONLY

This separate synthetic profile exercises the [release-driven candidate](../../../../docs/updates/public-channel-contract.md).
It is not imported by the app, package tools or a production signer. It sends no
HTTP requests, extracts/runs no installer, and generates no production keys.
All test signing seeds are openly derived from `PUBLIC ... NEVER PRODUCTION`
labels in the test file. Never use them as bootstrap or production material.

Run from the repository root with the existing Python/cryptography environment:

```sh
python3 -m unittest discover -s tests/fixtures/updates/release_manifest -p test_release_manifest.py -v
```

`release_manifest.py` reuses only historical SHA/canonical/signature-verification
helpers. The new schemas (`release-root-fixture-v1`, `release-manifest-fixture-v1`)
require `fixture_only=true`; they are not production metadata or TUF. Distinct
root/release keys and separate root/release thresholds model an embedded 2-of-3
root and owner 1-of-1 release policy. Tests create only inert byte strings labeled
synthetic, not real Linux ELF/Windows ZIP/app/resource evidence.

Checks cover both signed platform records and byte hashes, mutation, wrong
version/target/tag/source intent, strict stable identities/schema/JSON bounds,
latest→exact same-repo tag→direct/one-CDN supplied observations, key-role separation,
dual old/new sequential thresholds, duplicate/skipped/oversized rotation chains,
root and sequence/version floors through JSON state roundtrip, installed downgrade
refusal and exact raw pending-bundle retry. Unchanged already-installed metadata
is a normal no-update observation. Same sequence/version cannot change bytes.

The source SHA comparison models an independently reviewed release/build intent.
A signed source/compiler claim is not proof of compiled provenance. Artifact
app/descriptor hashes are signed assertions here; actual container/resource
validation belongs to the unsigned inspector and real reviewed build receipts.
`advance_policy` is exposed separately: a future client must durably persist its
authenticated result before release use, even when later data fails. This fixture
returns proposed public state; it does not implement crash-safe storage, cache
ownership, cancellation or filesystem trust protection.

One deliberate positive test accepts an older valid upgrade while a newer signed
release is withheld. **That is evidence of the unbounded freeze limitation, not a
freshness PASS.** There are no expiries, clock checks or recurring metadata work.
No real GitHub response, public project artifact, production signature/custody,
HTTP status/header behavior, installed/native acceptance or safe apply is proved.
Original four-role/public ZIP and foreign signing-request fixtures are untouched.
