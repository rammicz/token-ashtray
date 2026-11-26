# Developer-only offline update prototype — DRAFT / UNAPPROVED

**PUBLIC TEST KEYS. SYNTHETIC PAYLOADS. NEVER USE FOR PRODUCTION TRUST.**

Current update scope is [public anonymous GitHub Releases](../../../../docs/updates/public-channel-contract.md).
Patreon entitlement and denied-account requirements in the preserved research are
historical and superseded. These fixture sources and observed results remain
unchanged; [new public-channel examples](../public_channel/README.md) cover URL
boundaries and current ZIP resource tables without claiming real distribution.

These standalone developer scripts preserve the independently verified offline
slice of the update channel research. They are not
imported by the application or release packaging. No runtime dependencies,
service, account, updater preference or G6/G7 exception are introduced.
The historical distribution proposal, which is not part of the published source, remains unapproved;
current public anonymous download acceptance and remaining profile decisions stay
open. The only retained generated data in this directory is small JSON/text
evidence. Executables, ZIPs, generated metadata trees and generated seed files
are deliberately absent from the source repository.

## Regenerate outside the checkout

Run from the repository root. Use previously absent scratch directories;
generation refuses an existing destination. No network or listener is used.
The existing development environment needs Python 3, `cryptography`, GCC and
`x86_64-w64-mingw32-gcc`; OpenSSL is used for an independent crypto check.
Do not install these as application dependencies or enable hosted Actions.

```sh
python3 tests/fixtures/updates/offline_prototype/generate.py /tmp/token-ashtray-update-review-a
TOKEN_ASHTRAY_FIXTURE_DIRECTORY=/tmp/token-ashtray-update-review-a python3 -m unittest discover -s tests/fixtures/updates/offline_prototype -p test_contract.py -v
python3 tests/fixtures/updates/offline_prototype/verify.py /tmp/token-ashtray-update-review-a
openssl pkeyutl -verify -pubin -inkey /tmp/token-ashtray-update-review-a/openssl/targets-public.pem -rawin -in /tmp/token-ashtray-update-review-a/openssl/targets.signed -sigfile /tmp/token-ashtray-update-review-a/openssl/targets.sig
cmp tests/fixtures/updates/offline_prototype/evidence/reference-byte-manifest.json /tmp/token-ashtray-update-review-a/byte-manifest.json
cmp tests/fixtures/updates/offline_prototype/evidence/expected-vectors.json /tmp/token-ashtray-update-review-a/vectors.json
```

For a second independent generation and full-byte comparison:

```sh
python3 tests/fixtures/updates/offline_prototype/generate.py /tmp/token-ashtray-update-review-b
python3 tests/fixtures/updates/offline_prototype/check_identity.py /tmp/token-ashtray-update-review-a /tmp/token-ashtray-update-review-b
```

Tests require `TOKEN_ASHTRAY_FIXTURE_DIRECTORY` to name the generated tree.
Scripts do not fetch, extract, stage or execute the packaged binaries. To inspect
one vector, use `verify.py /tmp/token-ashtray-update-review-a --vector NAME`.
The expected result code for signed oversized artifacts is `artifact-size-limit`.

## Reproducibility and evidence

The recorded environment was Python 3.10.12, `cryptography` 3.4.8, OpenSSL 3.0.2,
GCC 11.4.0 and MinGW GCC 10-win32. On those tools, each generated tree has 99
files; two independent generations matched every byte, including 98 recorded
manifest entries and the manifest itself. Byte-manifest SHA-256:
`b65f268410bbfc8c9a3b9b8db5c7e65f2f1bb3a3bb30d3475cf246bef3cd9c03`.
Different compiler/library versions may produce different binary and archive
hashes; cross-toolchain-version reproducibility is not claimed.

The generated sources, payloads, role metadata, root chain, test keys and cases
are all covered by [reference-byte-manifest.json](evidence/reference-byte-manifest.json).
The manifest is a recorded observation; it is not a release signature or trust
anchor. [expected-vectors.json](evidence/expected-vectors.json) records 46 cases;
[observed-results.json](evidence/observed-results.json) records their exact actual
outcomes: 5 ACCEPT and 41 REJECT. Three unittest methods check artifact presence,
all expected outcomes and both oversized cases' signed length/hash binding.
[identity-evidence.json](evidence/identity-evidence.json),
[oversize-observations.json](evidence/oversize-observations.json),
and [openssl-result.txt](evidence/openssl-result.txt) retain the observed
evidence; the commands above reproduce the preserved contract from the repository.
[preservation-checks.json](evidence/preservation-checks.json) pins the repository
source scripts, original proposal bytes and successful repository-native rerun.

## Test data and narrow profile

The generator derives Ed25519 seeds from an explicit
`PUBLIC OFFLINE FIXTURE SEED; NEVER PRODUCTION` label. The algorithm and seeds
are public test data, not secrets. Generation writes a conspicuously named
`PUBLIC-TEST-SEEDS-NEVER-PRODUCTION.json` only to the scratch output. Root epoch 1
has 2-of-3 root keys and distinct 1-of-1 targets/snapshot/timestamp keys. Epochs
2 and 3 replace the complete key sets; each sequential root transition requires
both previous and next root thresholds. Root 1 is locally supplied test trust,
never downloaded and implicitly trusted. Duplicate signatures count once.

Synthetic versions are N=`0.2.1` and N+1=`0.2.2`; each executable embeds its own
version. The four positive ZIPs have distinct hashes preserved in the evidence.
Programs only print their `OFFLINE TEST FIXTURE ONLY` marker/version/target.
Linux payloads are genuine x86_64 ELF; Windows payloads are genuine x86_64 PE
cross-built with MinGW. **The `x86_64-pc-windows-msvc` metadata selector exercises
the proposed selection contract, not an MSVC-built Token Ashtray binary.**
There is no native Windows/application/installer/apply acceptance here.
Fixture source hashes are real C-source SHA-256 values; no repository source SHA
or production notices audit is asserted.

ZIPs use stored compression, sorted members, fixed 1980-01-01 timestamps and
Unix regular-file modes. Each has exactly three files under
`token-ashtray-{linux|windows}-x86_64-VERSION/`: executable, synthetic
`THIRD-PARTY.md` and `release-manifest.json`. `portable-zip-v1-fixture` and this
layout are provisional, not an adopted setup or updater extraction/resource contract.
Signed targets bind version, platform, purpose, format, runtime baseline,
artifact length/SHA-256, executable hash and synthetic notices hash.

This is **partial TUF-style support, not TUF conformance**. It exercises signed
root/targets/snapshot/timestamp roles, sequential root 1→2→3 rotation, expiry,
caller-supplied version floors, metadata hash/length/version bindings and exact
artifact hashes. JSON serialization is a limited compact sorted ASCII profile
with integer/string/list/object/bool/null values, not a general canonical JSON
implementation. There is no delegation, consistent-snapshot serving, full
untrusted-schema hardening, durable rollback store, root fetching/recovery,
trusted-clock handling or production TUF conformance suite.

The evaluation clock is fixed at `2026-11-21T12:00:00Z`. Runtime constraints are
compared with a fixture baseline (glibc 2.35 / Windows build 17763), not detected
on the host or approved as product support policy. Numeric stable SemVer rules
reject prereleases/leading zeros; build metadata cannot produce an upgrade at
equal precedence. ACCEPT means only offline verification of these fixture bytes.

The 41 rejections cover same/older/prerelease/build-only versions, malformed
version, target/purpose/kind/channel/runtime/URL mapping errors, both-platform
bit-flip/truncation/appending and correctly signed over-2-MiB artifacts,
internal manifest mismatch, ZIP traversal/symlink/duplicate members, each role's
rollback/expiry, modified metadata, unknown/revoked signing keys, mix-and-match,
freeze by expiry, skipped root, missing old/new root authorization, duplicate
root signature and expired final root. No authentic account, ACL, network request,
G6/G7 approval, token lifecycle, private source/archive readback, signing custody,
real release or updater client integration is established by these developer checks.
