"""PUBLIC TEST KEYS ONLY: golden vectors for the Rust update verifier (src/updates).

The bytes come from the release-manifest fixture's Python canonical serializer
and `cryptography` Ed25519 signer, so the Rust tests pin cross-language
agreement on canonical JSON, key IDs and signatures. Seeds are derived from the
same public labels as tests/fixtures/updates/release_manifest. Production
schema names are used because the app does not load `*-fixture-v1` schemas.
These vectors are never production trust material.

Regenerate from the repository root:

    python tests/fixtures/updates/client_vectors/generate.py
"""
import json
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / 'release_manifest'))
import release_manifest as profile  # noqa: E402
from test_release_manifest import key  # noqa: E402

canonical, sha, TARGETS, artifact_name = profile.canonical, profile.sha, profile.TARGETS, profile.artifact_name
HEADER = {'product': 'token-ashtray', 'repository': 'rammicz/token-ashtray', 'channel': 'stable'}
KEYS = {name: key(name) for name in ('old', 'root-a', 'root-b', 'root-c')}


def root_policy():
    roots = ('root-a', 'root-b', 'root-c')
    return {'schema': 'token-ashtray-release-root-v1', **HEADER, 'version': 1,
            'keys': {KEYS[name][0]: KEYS[name][1] for name in (*roots, 'old')},
            'roles': {'root': {'keyids': [KEYS[name][0] for name in roots], 'threshold': 2},
                      'release': {'keyids': [KEYS['old'][0]], 'threshold': 1}}}


def bundle(version='0.2.2'):
    rows = {}
    for target, kind in TARGETS.items():
        payload = ('SYNTHETIC NONEXECUTABLE ' + target + ' ' + version).encode()
        rows[target] = {'filename': artifact_name(target, version), 'purpose': 'installer',
                        'artifact_kind': kind, 'length': len(payload), 'sha256': sha(payload),
                        'app_sha256': 'b' * 64, 'descriptor_sha256': 'c' * 64}
    body = {'schema': 'token-ashtray-release-manifest-v1', **HEADER, 'version': version,
            'tag': 'v' + version, 'sequence': 2, 'root_version': 1, 'source_sha': 'a' * 40,
            'artifacts': rows}
    signature = {'keyid': KEYS['old'][0], 'sig': KEYS['old'][2].sign(canonical(body)).hex()}
    return {'manifest': {'signed': body, 'signatures': [signature]}, 'transitions': []}


CANONICAL_INPUT = (b'{ "z" : [0, 9007199254740991, true, false, ""],\n'
                   b'  "a": {"q": "say \\"hi\\" \\\\ \\/ \\u0041~"}, "m": [ ] }\n')

if __name__ == '__main__':
    outputs = {
        'root-v1.json': canonical(root_policy()),
        'bundle-0.2.2.json': canonical(bundle()),
        'canonical-input.json': CANONICAL_INPUT,
        'canonical-expected.json': canonical(json.loads(CANONICAL_INPUT)),
    }
    for name, data in outputs.items():
        (HERE / name).write_bytes(data)
