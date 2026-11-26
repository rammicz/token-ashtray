# Optional public updates — release-driven candidate

The owner approved anonymous opt-in updates from canonical
`rammicz/token-ashtray` GitHub Releases and still holds production keys/approves
new releases. The owner's 2026-11-22 clarification rejects recurring
operator maintenance. The active candidate publishes an
immutable signed manifest with each approved release: no weekly review,
calendar key renewal, timestamp service or new online signer. Idle months need
no metadata work. Actions remain disabled and publication stays owner-controlled.

The [release contract](public-channel-contract.md) defines the single exact
`releases/latest/download/token-ashtray-updates.json` discovery entry, same-repo
stable-tag hop and direct/one exact CDN boundary. [G7](../REQUIREMENTS.md#g7-network-scope-and-implementation-status)
remains default-off; after explicit enable, the update client checks/downloads
automatically and applies after Quit. No login/token/cookies/telemetry or
arbitrary URL is added. That client embeds no production root, so it reports
updates unavailable and sends no request. No production signer exists, and
native acceptance of the update path remains open.

The [owner operations](owner-signing-operations.md) separate offline root
transition authority from ordinary release approval, bind frozen requests and
signed responses, and describe event-only custody/recovery/publication.
The [new release-manifest fixture](../../tests/fixtures/updates/release_manifest/README.md)
checks signatures, exact identities/redirect observations, root/sequence/version
floors and identical pending retry with public test keys/synthetic bytes.

**Freeze limitation:** GitHub discovery can withhold or replay an old valid
release indefinitely. This design has no cryptographic time-based freshness;
first observation may accept an older authorized upgrade despite a hidden newer
one. Stored floors prevent later rollback, not suppression of unseen releases.
This tradeoff and the exact production profile still require review.

The historical [four-role TUF-style prototype](../../tests/fixtures/updates/offline_prototype/README.md)
and [public ZIP examples](../../tests/fixtures/updates/public_channel/README.md)
retain their original evidence, not active expiry/entitlement/service
requirements. Existing signing-request fixture work
remains separate. [Package compatibility constraints](package-compatibility-addendum.md)
and the unsigned inspector still inform real artifact verification; old transport/
renewal assumptions in historical research do not override the current contract.

Approved bootstrap/custody, real signed project assets and anonymous
both-platform download/tamper evidence remain required before updates are
enabled. No public project manifest or package is proved by an unrelated GitHub
probe or these offline fixtures. Windows update apply still needs native QA.
