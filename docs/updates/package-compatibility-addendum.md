# DRAFT / UNAPPROVED — package compatibility addendum

Current channel scope: [public anonymous GitHub Releases](public-channel-contract.md).
The historical private-channel/entitlement references below are superseded by
that decision; the resource inventory and apply constraints remain applicable.

This addendum reconciles the earlier distribution proposal, which is not part
of the published source, with the first delivered Linux graphical setup.
It records constraints for a future approved update contract, not a production
schema or permission to implement an updater. G6/G7, the original research and the
synthetic fixtures remain unchanged. [RELEASE.md](../RELEASE.md) remains the
manual delivery procedure; no service or private channel is configured here.

## Observed incompatibilities

| Existing format | Exact resources and layout | Compatibility gap |
| --- | --- | --- |
| [Portable ZIP packager](../../scripts/package_release.py) | Flat common members: executable, `LICENSE`, `THIRD-PARTY.md`, `LICENSE-REVIEW.md`, `RELEASE-METRICS.json`. Linux adds `INSTALL-LINUX.md`, `install-linux.py`, `token-ashtray-256.png` (8 total); Windows adds `INSTALL-WINDOWS.md`, `install-windows.ps1` (7 total). | No fixture `release-manifest.json` or versioned directory; the Linux ZIP has no graphical setup executable. |
| [Linux setup packager](../../scripts/package_linux_setup.py) and [installation code](../../src/linux_setup.rs) | Appended ELF payload has seven members: `token-ashtray`, `token-ashtray-256.png`, `LICENSE`, `THIRD-PARTY.md`, `LICENSE-REVIEW.md`, `INSTALL-LINUX.md`, `RELEASE-METRICS.json`. | Installation also writes the complete downloaded ELF as `token-ashtray-setup`, `SETUP-MANIFEST.json` (payload/stub identity), and `.token-ashtray-install`; it creates app and removal launchers. Replacing only the app does not define the next coherent installation. |
| [Windows graphical setup](../../scripts/build_windows_setup.py) (`windows-setup-exe-v1`) | Per-user Inno Setup EXE that embeds the seven portable ZIP members plus `.token-ashtray-install`, `.setup\.token-ashtray-setup` and `INNO-LICENSE.txt`; notices, guide, helper and measurements go to `.setup`. | Accepted as the Windows update artifact. The ZIP helper replaces only the app, marker and shortcut, so applying the ZIP to a setup installation would leave stale `.setup` resources and Installed apps version. [artifact_descriptor.py](../../scripts/artifact_descriptor.py) inspects the setup's PE32 GUI stub beside its input ZIP; that build relation is a caller assertion. |
| [Historical offline verifier](../../tests/fixtures/updates/offline_prototype/verify.py) | Exactly executable, synthetic `THIRD-PARTY.md`, `release-manifest.json` under a versioned directory; `portable-zip-v1-fixture`; 2 MiB artifact/member/expanded-total limits. | Synthetic packages omit current licenses, measurements, guides, icon and setup/removal resources. The fixture limits are not measured release limits. |

An offline inspection on 2026-11-21 used the existing `PackageResources.package`
helper with disposable synthetic executables and the real ZIP packager. Both
8/7-member archives were rejected by the historical `verify_package` with
`zip-layout`; correctly length/hash-bound 2,097,153-byte inputs were rejected
with `artifact-size-limit` on both selectors. These observations demonstrate
incompatibility, not a broken fixture or signed real-release acceptance. The
previously recorded app from the first Linux setup alone was 18,197,560 bytes; its size is historical,
not a measurement of this baseline. Keep the original 46 vectors and their
evidence intact; add a separate approved profile and new fixtures later.

## Constraints to resolve before integration

The following are proposed acceptance constraints for the next contract:

- Specify exact platform, version, source SHA, purpose, package kind and schema.
  Keep installer and update purposes distinct; neither the standalone setup ELF
  nor a current portable ZIP becomes an update merely by renaming it or changing
  a metadata label. Reject unsupported formats before staging.
- Define the entire next installed resource set and which resources are shipped,
  generated, retained or removed. Bind the signed artifact length/hash to an
  internal manifest with each shipped member's path, byte length, hash, file
  type and target permissions. Bind app, generated notices, license review and
  measurements to the same approved build; `RELEASE-METRICS.json` must identify
  that app hash. Record compatible versions/hashes and an explicit reason for
  every retained resource rather than silently leaving stale installation data.
- For Linux, cover all seven setup payload members, the complete installed
  `token-ashtray-setup` self-copy, `SETUP-MANIFEST.json`, ownership marker and
  both XDG menu entries (`token-ashtray.desktop` and
  `token-ashtray-setup.desktop`). Preserve a working graphical removal path and
  the manifest's actual stub/payload identity. Specify how these are refreshed
  or prove compatibility for retained bytes; the existing ZIP lacks the setup
  bytes needed to assume this. Preserve configuration, provider credentials and
  enabled/disabled autostart, including XDG overrides and owned-entry checks.
- Choose measured compressed-artifact, per-member, expanded-total and member-count
  bounds for each approved platform format. Keep metadata limits separate.
  Exercise both a genuine app-sized accepted package and correctly signed
  oversized rejection. Raising the prototype's single 2 MiB constant alone
  cannot establish extraction safety or release compatibility.
- Define exact path/layout and permission rules and reject missing, unexpected,
  duplicate, traversal, absolute, symlink and other non-regular members before
  mutation. Never ship source archives, local config or credentials. A valid
  signature does not authorize arbitrary filesystem paths or execution.

## Handoff and remaining decisions

The public channel decision retains review of exact G7
wording/request inventory, paths/redirect boundary, signing custody and expiry/renewal.
The owner approved the GitHub Releases source and anonymous opt-in scope; G6 remains
unchanged. No update entitlement service or user login is needed.
The package kind/schema, resource migration strategy and measured limits also
need an agreed specification. This addendum selects none of those choices.
After approval, produce signed test artifacts from actual approved Linux and
Windows builds, verify member/build identities and tamper rejection, and retain
anonymous download and tamper-rejection evidence plus public distribution
configuration readback. Repository conversion/publication stays owner-controlled.
Offline fixture success satisfies none of those live criteria.

The Linux update client must consume the agreed
contract and prove Linux ZIP-install and GUI-install N→N+1 upgrades, removal,
quit/restart handoff, ownership/lock conflicts, failure rollback and retry. Verify
all installed resource identities and unchanged config/autostart/provider canaries
after success and injected commit failures; downloaded/verified bytes alone are
not apply acceptance. Reconcile with the Linux setup's staged writes, stable-parent lock and
recovery behavior before choosing an apply mechanism.

Windows replacement, installer/resource handoff and native Windows evidence
follow [WINDOWS-ACCEPTANCE.md](../WINDOWS-ACCEPTANCE.md). The synthetic MinGW fixture,
Linux checks and packaging inspection establish no native Windows acceptance.
This document grants no network, deployment, signing or publication authority.
