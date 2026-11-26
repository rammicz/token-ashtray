# Public GitHub Releases update contract — release-driven candidate

## Decision and present state

The owner approved canonical `rammicz/token-ashtray` GitHub Releases, anonymous
opt-in client updates, and owner-held production keys/release approval. On
2026-11-22 the owner rejected recurring operator maintenance, including the
previous weekly renewal candidate. The active proposal is an immutable signed
manifest published with each approved application release. Idle months require
no metadata renewal, signing job or online signer. Private material stays outside
GitHub and this session; Actions remain disabled.

This is a custom signed-release protocol, **not TUF**. The earlier four-role
profile and its expiry schedules are superseded review history. The
[historical fixtures](../../tests/fixtures/updates/offline_prototype/README.md)
remain unchanged as partial TUF-style evidence, not the active production plan.
The [owner operations](owner-signing-operations.md) describe event-only custody,
approval and recovery. Exact profile acceptance and actual public signed assets
remain unverified, and native Windows acceptance is still required. This
contract introduces no production signer, signing keys or public delivery. The
update client implements it without a production root, so no build makes an
update request yet.

## Automation and G7 boundary

G6 and [G7](../REQUIREMENTS.md#g7-network-scope-and-implementation-status) remain
unchanged: updates start off. Explicit enable will explain source and request
data, then allow automatic checks and verified downloads. The candidate cadence
is an immediate check followed by at most one scheduled check per 24 hours;
Check now uses the same bounded inventory. Disable cancels checks/downloads,
prevents prepared apply and stops further updater requests. The Linux and
Windows clients must prove Quit/restart, safe apply, rollback/retry and
preserved config/provider/autostart state. A valid signature does not itself authorize execution or installation.

Requests are anonymous HTTPS GETs, with application/version User-Agent and
appropriate Accept. No login, account, Authorization, cookie, Referer, provider
data, machine identifier, telemetry or repository token. GitHub/CDN sees IP,
version, time and requested platform. CDN query signatures are transport data;
do not persist them in normal logs. Opening settings/About or reading providers
sends no updater request. Current explicit author-feed/link behavior is separate.
Use a 30-second request timeout and one bounded retry for temporary failures;
no background retry after Disable. Missing/failed discovery leaves the installed
app and durable trust state intact.

## Exact discovery and redirect graph

The sole discovery URL is:

`https://github.com/rammicz/token-ashtray/releases/latest/download/token-ashtray-updates.json`

GitHub [documents latest-asset download links](https://docs.github.com/en/repositories/releasing-projects-on-github/linking-to-releases#linking-to-the-latest-release).
This is an explicit revision of the earlier exact-path candidate that excluded
`/latest`; it does not broaden G7 to arbitrary GitHub URLs or REST API polling.

Require the first hop to the same repository's exact stable-tag manifest URL:

`https://github.com/rammicz/token-ashtray/releases/download/v<X.Y.Z>/token-ashtray-updates.json`

That tag must match the signed manifest's strict stable version. From that
versioned URL allow direct bytes or one HTTPS hop to exactly
`release-assets.githubusercontent.com`, default port, no URL credentials or
fragment. Thus discovery allows at most two redirects total. Artifact requests
start at their constructed versioned URL and allow direct bytes or that single
CDN hop. GitHub's [asset documentation](https://docs.github.com/en/rest/releases/assets#get-a-release-asset)
describes direct/redirected downloads. Representative unrelated GitHub behavior
corroborates the graph, not this project's hosting or public availability.

Initial/canonical URLs have no query, fragment or userinfo. Accept only the exact
repository/tag/filename grammar below; no caller/manifest-supplied URL, alternate
host/port, wildcard/suffix matching, source archive, HTML scraping or fallback
endpoint. The CDN's returned opaque path/query is used solely for that observed
request and never becomes a metadata authority or reusable caller input. Reject
other or additional hops; fail closed and review an actual project mismatch.
The fixture checks supplied observations, not HTTP statuses/headers/cancellation.

| Asset | Exact canonical path after `https://github.com` |
| --- | --- |
| Manifest | `/rammicz/token-ashtray/releases/download/v<X.Y.Z>/token-ashtray-updates.json` |
| Linux x86_64 installer | `/rammicz/token-ashtray/releases/download/v<X.Y.Z>/token-ashtray-setup-<X.Y.Z>-x86_64-unknown-linux-gnu` |
| Windows x64 per-user setup | `/rammicz/token-ashtray/releases/download/v<X.Y.Z>/token-ashtray-setup-<X.Y.Z>-x86_64-pc-windows-msvc.exe` |

## Signed manifest and trust policy

Candidate production schemas are `token-ashtray-release-manifest-v1` and
`token-ashtray-release-root-v1`. The fixed-name asset contains a `manifest`
signed envelope plus bounded signed `transitions`. Each envelope has a `signed`
object and distinct `{keyid,sig}` signatures. Use Ed25519, SHA-256, and a pinned
compact sorted-key ASCII JSON profile with integers/booleans/strings/arrays/objects
only; no floats, duplicate object keys or unknown schema fields. The executable
fixture uses different fixture-only schemas and public test keys, never production
trust. Pin/review the real implementation and exact serialization before adoption.

Embed an owner-verified initial public root-policy blob/hash in the client.
Propose separate offline 2-of-3 root authority and a 1-of-1 owner release key;
keys/thresholds and the public bootstrap still require actual verification.
Root policy identifies product, repository, channel, increasing policy version,
Ed25519 public keys and disjoint root/release role key IDs/thresholds. At most
three keys per role; root threshold must remain at least two. Release keys cannot
authorize a policy transition; root signatures cannot directly substitute for
a release signature. Ordinary manifests contain only authorized release-key
signatures; duplicate IDs are rejected. No role has a calendar expiry or renewal
requirement.

The signed release payload binds product/repository/channel `stable`, strict
numeric `X.Y.Z` (no prerelease/build metadata/leading zeros), exact `vX.Y.Z` tag,
positive increasing release authorization sequence, current root-policy version,
full source SHA, and exactly both supported platform rows. Each row binds the
constructed filename, `purpose=installer`, artifact kind, exact size/SHA-256,
app hash and resource-descriptor hash. Reject unsupported targets/formats/purposes
and conflicting mappings. Source/compiler/resource provenance is established by
reviewed build receipts, not inferred from a header or signed assertion alone.

Linux remains `linux-setup-elf-v1`. Windows uses `windows-setup-exe-v1`: the
per-user Inno Setup executable built by `scripts/build_windows_setup.py` from the
validated portable ZIP of the same approved build (AppId
`{78FA49B8-BA85-4D80-97DC-C1C1A75F1FE7}`, `PrivilegesRequired=lowest`). The
owner accepted this kind on 2026-11-24. The client runs only the verified
bytes, unattended, after every app process has exited (the single-instance guard object is
absent), with `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /NOCANCEL /SP-
/DIR=<current install> /LOG=<state dir> /WAITAPP`. Setup keeps its ValidateOnly
checks and file-transaction rollback. `app_sha256` and `descriptor_sha256` are
signed release-receipt assertions; the client checks only their shape for this
kind. The portable ZIP remains a manual download and is not an update artifact.
Keep the [complete resource/apply constraints](package-compatibility-addendum.md):
seven Linux payload resources plus installed setup/identity/removal coherence,
and the seven Windows ZIP members the setup embeds. Bind each resource's exact
path/type/mode/size/hash and metrics to the same approved build. Reject
missing/extra/duplicate/traversal/absolute/symlink/non-regular members. The
platform clients own the safe installer handoff.

Candidate bounds, pending actual final-build measurement: 64 KiB for the complete
manifest/rotation bundle, eight retained transition records per bundle, artifact
128 MiB, expanded payload 128 MiB total and 64 MiB per member. Enforce declared
and actual bytes. Retain every transition in release records; if the bounded
bundle cannot bridge an old client's trust epoch, report unavailable/manual
verified bootstrap recovery. Never skip a root, fetch arbitrary key URLs or
silently raise bounds. No automatic downloaded trust-on-first-use.

## Stored floors, retry and freeze limitation

Transitions advance exactly N→N+1, signed by distinct keys meeting both the old
root threshold and the new root's own threshold. Bound the complete chain;
retain old records for stale clients. Persist authenticated root advancement
before authorizing releases, even if subsequent release/download verification
fails. Invalid later data cannot erase the established root floor. The fixture
exposes separate policy advancement; real durable/crash-safe storage is work
for the shared update client.

Store trusted root policy, maximum release sequence/version and exact accepted
bundle digest outside disposable staging/cache. Lower root/sequence/version is
rejected. The same sequence cannot change any bundle bytes; only the exact known
pending digest may retry a download. A new sequence must advance version; changing
bytes under the same immutable release version/tag is prohibited. Installed
version can never decrease. An exact already-installed known release is a normal
no-update observation, not permission to reapply it. Preserve floors through
restart/cancellation and verify artifact bytes before any handoff.

**No cryptographic time-based freshness is provided.** HTTPS GitHub discovery
can be suppressed or replayed indefinitely. A first observation may accept an
old valid upgrade while a newer owner-approved release is hidden; later stored
floors prevent only rollback below what that client has already accepted.
Signed dates, if later added for display, would not prove current freshness.
There is no timestamp service, periodic metadata signing, expiry clock or claim
that a client is cryptographically current. Say “no newer verified release was
found,” and preserve ordinary app use when updating is unavailable. This freeze
tradeoff requires explicit profile review before production acceptance.

## Owner-approved publication and live acceptance

For each new release, the owner approves the frozen source/build/artifact intent
and signs the immutable manifest (and any event-driven root transitions) outside
GitHub/session. Package both actual platform builds, notices, metrics and receipt
identities at one reviewed source SHA. Verify the complete signed set offline.
Upload both artifacts and manifest into the same version-tag draft, verify every
byte, then publish that release and its intended latest designation together.
No `updates-stable` control release, timestamp replacement or idle-month signing
job is required. Publishing a tag/source push alone still creates no packages
while Actions are paused. Visibility/publication remains an owner action.

Require actual repository/release/asset readback, anonymous discovery through the
exact graph, both real platform downloads and signature/hash/tamper rejection.
A missing asset, interrupted publication, 404, unsupported redirect or replay
must not execute an update or damage installed/trusted state. Use no auth/cookies/
netrc/browser session in anonymous acceptance, and omit CDN query values from
retained evidence. Native installation/provider acceptance is separate. No real
Token Ashtray signed manifest or public artifact is verified by this source work.

## Offline evidence and unsigned inventory

The [new release-manifest fixture](../../tests/fixtures/updates/release_manifest/README.md)
executes the custom candidate with public test keys, inert artifact bytes and
synthetic stored state. It proves selected signature/identity/redirect/rotation/
rollback/retry rules and deliberately demonstrates acceptance of an old valid
upgrade under withholding. It is not a production client, signer, HTTP stack,
package parser or crash-safe durable store.

The [historical public ZIP examples](../../tests/fixtures/updates/public_channel/README.md)
and [46-vector TUF-style prototype](../../tests/fixtures/updates/offline_prototype/README.md)
retain their original schemas/keys/serializers and limited evidence. Their
four-role expiry and former control-path assumptions do not govern this profile.
Foreign signing-request fixtures remain separate; no shared fixture is changed.

`python3 scripts/artifact_descriptor.py --help` describes the unsigned offline
inspector for current Linux setup ELF/Windows ZIP resources, app/metrics, stub and
raw trailer identities. Reference/source/version/compiler provenance remains
caller asserted until actual reviewed build receipts close it. Its pretty JSON
is not a production signing canonicalizer. Run `python3 tests/test_artifact_descriptor.py`
for its isolated package checks; no signature, native or live acceptance follows.
