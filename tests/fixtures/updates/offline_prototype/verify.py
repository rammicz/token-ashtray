"""TEST ONLY: narrow TUF-style offline verifier, NOT a TUF client or updater."""
import argparse
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import re
import stat
import zipfile

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

NOW = '2026-11-21T12:00:00Z'
TARGETS = {
    'x86_64-unknown-linux-gnu': ('linux', 'token-ashtray', {'glibc': '2.35'}),
    'x86_64-pc-windows-msvc': ('windows', 'token-ashtray.exe', {'windows_build': 17763}),
}
LIMIT = 2 * 1024 * 1024


class Reject(Exception):
    pass


def require(condition, code):
    if not condition:
        raise Reject(code)


def canonical(value):
    # This limited profile permits integer/string/list/object/bool/null only.
    # The generator uses printable ASCII; general TUF canonical JSON is NOT claimed.
    return json.dumps(value, sort_keys=True, separators=(',', ':'),
                      ensure_ascii=True, allow_nan=False).encode('ascii')


def sha(data):
    return hashlib.sha256(data).hexdigest()


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, 'duplicate-json-key')
        result[key] = value
    return result


def load(data):
    require(len(data) <= LIMIT, 'metadata-size')
    return json.loads(data, object_pairs_hook=unique_object)


def signed_by(envelope, trusted, role):
    rule = trusted['roles'][role]
    allowed = set(rule['keyids'])
    threshold = rule['threshold']
    require(type(threshold) is int and 1 <= threshold <= len(allowed), 'threshold')
    require(len(allowed) == len(rule['keyids']), 'duplicate-role-key')
    valid = set()
    for signature in envelope['signatures']:
        kid = signature['keyid']
        if kid not in allowed or kid in valid:
            continue
        key = trusted['keys'][kid]
        require(key['keytype'] == key['scheme'] == 'ed25519', 'key-algorithm')
        require(sha(canonical(key)) == kid, 'keyid')
        try:
            Ed25519PublicKey.from_public_bytes(bytes.fromhex(key['keyval']['public'])).verify(
                bytes.fromhex(signature['sig']), canonical(envelope['signed']))
            valid.add(kid)
        except (InvalidSignature, ValueError):
            pass
    require(len(valid) >= threshold, 'signature-' + role)


def header(signed, role, now, floor=0):
    require(signed['_type'] == role and signed['spec_version'] == '1.0.0', 'metadata-type')
    require(signed['fixture_only'] is True, 'fixture-marker')
    require(type(signed['version']) is int and signed['version'] >= 1, 'metadata-version')
    require(signed['version'] >= floor, 'rollback-' + role)
    require(bool(re.fullmatch(r'\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z', signed['expires'])),
            'expiry-format')
    require(signed['expires'] > now, 'expired-' + role)


def roots(bootstrap, chain, now):
    current_envelope = load(bootstrap)
    current = current_envelope['signed']
    signed_by(current_envelope, current, 'root')
    # The initial root bytes are locally supplied trust, never network TOFU.
    for data in chain:
        candidate = load(data)
        require(candidate['signed']['version'] == current['version'] + 1, 'root-sequence')
        signed_by(candidate, current, 'root')
        signed_by(candidate, candidate['signed'], 'root')
        current = candidate['signed']
    header(current, 'root', now)
    return current


def descriptor(data, expected):
    require(len(data) == expected['length'], 'metadata-length')
    require(sha(data) == expected['hashes']['sha256'], 'metadata-hash')
    require(load(data)['signed']['version'] == expected['version'], 'metadata-version-binding')


def stable(version):
    match = re.fullmatch(r'(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)'
                         r'(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?', version)
    require(match is not None, 'not-stable-semver')
    return tuple(int(match.group(i)) for i in (1, 2, 3))


def verify_package(data, entry, version, target):
    require(len(data) <= LIMIT, 'artifact-size-limit')
    require(len(data) == entry['length'], 'artifact-length')
    require(sha(data) == entry['hashes']['sha256'], 'artifact-hash')
    platform, executable, _ = TARGETS[target]
    prefix = f'token-ashtray-{platform}-x86_64-{version}'
    expected = {f'{prefix}/{name}' for name in (executable, 'THIRD-PARTY.md', 'release-manifest.json')}
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        infos = archive.infolist()
        names = [info.filename for info in infos]
        require(len(names) == len(set(names)), 'duplicate-zip-member')
        for info in infos:
            path = PurePosixPath(info.filename)
            require(not path.is_absolute() and '..' not in path.parts and
                    '\\' not in info.filename and ':' not in info.filename, 'unsafe-zip-path')
            require(stat.S_ISREG(info.external_attr >> 16), 'unsafe-zip-type')
            require(info.file_size <= LIMIT, 'zip-member-size')
        require(sum(info.file_size for info in infos) <= LIMIT, 'zip-total-size')
        require(set(names) == expected, 'zip-layout')
        binary = archive.read(f'{prefix}/{executable}')
        notice = archive.read(f'{prefix}/THIRD-PARTY.md')
        manifest = load(archive.read(f'{prefix}/release-manifest.json'))
        require(manifest['fixture_only'] is True and manifest['version'] == version and
                manifest['target'] == target, 'package-identity')
        require(sha(binary) == entry['custom']['binary_sha256'] == manifest['binary_sha256'],
                'package-binary-hash')
        require(sha(notice) == entry['custom']['notices_sha256'] == manifest['notices_sha256'],
                'package-notices-hash')
        if platform == 'linux':
            require(binary[:5] == b'\x7fELF\x02' and int.from_bytes(binary[18:20], 'little') == 62,
                    'binary-platform')
        else:
            require(binary[:2] == b'MZ', 'binary-platform')
            pe = int.from_bytes(binary[60:64], 'little')
            require(binary[pe:pe + 6] == b'PE\0\0\x64\x86', 'binary-platform')


def verify(bootstrap, root_chain, metadata, artifact_loader, target,
           current_version, now=NOW, floors=None):
    require(target in TARGETS, 'unsupported-target')
    root = roots(bootstrap, root_chain, now)
    floors = floors or {}
    bodies = {}
    for role in ('timestamp', 'snapshot', 'targets'):
        envelope = load(metadata[role])
        signed_by(envelope, root, role)
        header(envelope['signed'], role, now, floors.get(role, 0))
        bodies[role] = envelope['signed']
    descriptor(metadata['snapshot'], bodies['timestamp']['meta']['snapshot.json'])
    descriptor(metadata['targets'], bodies['snapshot']['meta']['targets.json'])
    matches = [(path, entry) for path, entry in bodies['targets']['targets'].items()
               if entry['custom']['target'] == target]
    require(len(matches) == 1, 'target-mapping')
    path, entry = matches[0]
    custom = entry['custom']
    require(custom['product'] == 'token-ashtray' and custom['channel'] == 'stable', 'product-channel')
    require(custom['fixture_only'] is True, 'fixture-marker')
    require(custom['purpose'] == 'update' and custom['artifact_kind'] == 'portable-zip-v1-fixture',
            'package-purpose')
    require(custom['minimum_runtime'] == TARGETS[target][2], 'runtime-constraint')
    version = custom['version']
    require(stable(version) > stable(current_version), 'no-upgrade')
    platform = TARGETS[target][0]
    require(path == f'stable/{target}/{version}/token-ashtray-{platform}.zip', 'artifact-path')
    verify_package(artifact_loader(path), entry, version, target)
    # Returning is evidence of validated bytes only; there is no extraction/stage/apply here.
    return {'outcome': 'ACCEPT', 'code': 'verified', 'version': version,
            'target': target, 'artifact_sha256': entry['hashes']['sha256'],
            'root_version': root['version'],
            'metadata_versions': {role: bodies[role]['version'] for role in bodies}}


def run_vector(directory, vector):
    release = directory / 'releases' / vector.get('release', 'n-plus-one')
    metadata = {role: (directory / vector.get('metadata', {}).get(
        role, str((release / (role + '.json')).relative_to(directory)))).read_bytes()
                for role in ('timestamp', 'snapshot', 'targets')}
    def artifact(path):
        override = vector.get('artifacts', {}).get(path)
        return (directory / override if override else release / 'artifacts' / path).read_bytes()
    try:
        return verify((directory / 'bootstrap/1.root.json').read_bytes(),
                      [(directory / path).read_bytes() for path in vector.get(
                          'roots', ['roots/2.root.json', 'roots/3.root.json'])],
                      metadata, artifact, vector['target'], vector.get('current', '0.2.1'),
                      vector.get('now', NOW), vector.get('floors'))
    except Reject as exc:
        return {'outcome': 'REJECT', 'code': str(exc)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('--vector')
    args = parser.parse_args()
    vectors = json.loads((args.directory / 'vectors.json').read_text())
    results = []
    for vector in vectors:
        if args.vector and vector['name'] != args.vector:
            continue
        result = run_vector(args.directory, vector)
        result['name'] = vector['name']
        result['expected'] = vector['expected']
        result['expected_code'] = vector['code']
        results.append(result)
    require(bool(results), 'unknown-vector')
    print(json.dumps(results, indent=2, sort_keys=True))
    return 0 if all(r['outcome'] == r['expected'] and r['code'] == r['expected_code']
                    for r in results) else 1


if __name__ == '__main__':
    raise SystemExit(main())
