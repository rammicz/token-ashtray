# Public-channel offline examples — test keys, synthetic bytes only

This developer-only profile addresses URL/redirect and current ZIP resource-binding
gaps identified in the [public channel review draft](../../../../docs/updates/public-channel-contract.md).
It performs no network request, extraction, process launch or install. It is not
imported by the app or release packagers. PUBLIC TEST KEYS MUST NEVER SIGN RELEASES.

Python 3 and `cryptography` are development prerequisites already used by the
[historical prototype](../offline_prototype/README.md); no compiler is needed here.
Run at the repository root, using previously absent scratch destinations:

```sh
python3 -m unittest discover -s tests/fixtures/updates/public_channel -p test_public_contract.py -v
python3 tests/fixtures/updates/public_channel/public_contract.py /tmp/token-ashtray-public-example-a
python3 tests/fixtures/updates/public_channel/public_contract.py /tmp/token-ashtray-public-example-b
python3 tests/fixtures/updates/offline_prototype/check_identity.py /tmp/token-ashtray-public-example-a /tmp/token-ashtray-public-example-b
```

Each generation has five files: two ZIPs, two signed targets envelopes and one
byte manifest. Fixed ZIP timestamps/order/modes, source resource bytes and a public
deterministic Ed25519 seed make generations from the same source/toolchain equal.
The embedded trust key is supplied locally, never downloaded. It reuses historical
signature, expiry and stable-version verification helpers without altering them.
The evaluation clock is the historical fixed `2026-11-21T12:00:00Z`, not host time.
[evidence.json](evidence.json) records the observed five-file identity/hash results
and separate anonymous external GitHub feature probe, with their scope limits.

ZIPs have exactly the current flat 8 Linux / 7 Windows packager resources and
target modes, copied from repository resources; each executable is an explicitly
nonexecutable 3 MiB synthetic byte string, and metrics explicitly report no real
measurements. Signed member tables bind every path, length, hash and mode plus
artifact hash/length, version, target, purpose and candidate versioned URL. The
example purpose is `packaging-example`, schema `public-packaging-example-v1`, with
an example-only 16 MiB bound. None may be treated as a production feed or package.

Four tests cover accepted direct/CDN transport observations, hostile initial URLs
and redirects, signature/byte tampering on both platforms, same-version and wrong
target rejection, authentically signed missing/wrong resource descriptors, and
two identical generations. They do not fetch actual GitHub URLs or prove request
headers, anonymous access, complete schema hardening or TUF conformance. The
historical 46 vectors retain broader root/role rotation and rollback evidence.
These ZIP examples do not model the Linux setup's appended manifest or complete
installed resource migration; the Linux and Windows update clients still need real artifacts and apply evidence.
