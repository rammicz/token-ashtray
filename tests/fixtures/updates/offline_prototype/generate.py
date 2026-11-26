"""Generate deterministic PUBLIC TEST KEY fixtures. Never use these keys in production."""
import argparse
import copy
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import zipfile

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from verify import canonical, sha, TARGETS, LIMIT

FIXED = (1980, 1, 1, 0, 0, 0)
SEED_WARNING = 'PUBLIC OFFLINE FIXTURE SEED; NEVER PRODUCTION'
KEYS = {}
SEEDS = {}


def key(label):
    seed = hashlib.sha256((SEED_WARNING + ':' + label).encode()).digest()
    private = Ed25519PrivateKey.from_private_bytes(seed)
    public = private.public_key().public_bytes(serialization.Encoding.Raw,
                                             serialization.PublicFormat.Raw)
    description = {'keytype': 'ed25519', 'scheme': 'ed25519', 'keyval': {'public': public.hex()}}
    kid = sha(canonical(description))
    KEYS[kid] = (private, description)
    SEEDS[label] = {'keyid': kid, 'public_test_seed_hex': seed.hex()}
    return kid


def sign(signed, keyids):
    return {'signed': signed, 'signatures': [
        {'keyid': kid, 'sig': KEYS[kid][0].sign(canonical(signed)).hex()} for kid in keyids]}


def write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data if isinstance(data, bytes) else canonical(data) + b'\n')


def base(role, version):
    return {'_type': role, 'version': version, 'spec_version': '1.0.0', 'fixture_only': True,
            'expires': {'root': '2027-11-21T12:00:00Z', 'targets': '2027-02-19T12:00:00Z',
                        'snapshot': '2026-11-28T12:00:00Z',
                        'timestamp': '2026-11-22T12:00:00Z'}[role]}


def root(version):
    body = base('root', version)
    roles = {}
    for role in ('root', 'targets', 'snapshot', 'timestamp'):
        ids = [key(f'epoch-{version}-{role}-{index}') for index in range(3 if role == 'root' else 1)]
        roles[role] = {'keyids': ids, 'threshold': 2 if role == 'root' else 1}
    body.update({'keys': {kid: KEYS[kid][1] for rule in roles.values() for kid in rule['keyids']},
                 'roles': roles, 'consistent_snapshot': False})
    return body


def root_signers(body):
    return body['roles']['root']['keyids'][:2]


def zip_bytes(members):
    data = io.BytesIO()
    with zipfile.ZipFile(data, 'w', compression=zipfile.ZIP_STORED) as archive:
        for name, content, mode in sorted(members):
            info = zipfile.ZipInfo(name, FIXED)
            info.create_system = 3
            info.external_attr = mode << 16
            info.compress_type = zipfile.ZIP_STORED
            archive.writestr(info, content)
    return data.getvalue()


def compile_payload(directory, version, target):
    platform, executable, _ = TARGETS[target]
    location = directory / 'build' / version / target
    location.mkdir(parents=True)
    source = ('#include <stdio.h>\nint main(void) {\n'
              f'  puts("TOKEN ASHTRAY OFFLINE TEST FIXTURE ONLY {version} {target}");\n'
              '  return 0;\n}\n').encode()
    source_path = location / 'fixture.c'
    source_path.write_bytes(source)
    binary_path = location / executable
    compiler = 'gcc' if platform == 'linux' else 'x86_64-w64-mingw32-gcc'
    flags = ['-Wl,--build-id=none'] if platform == 'linux' else ['-Wl,--no-insert-timestamp']
    command = [compiler, '-Os', '-s', *flags, str(source_path), '-o', str(binary_path)]
    subprocess.run(command, check=True, env={**os.environ, 'SOURCE_DATE_EPOCH': '315532800',
                                            'LC_ALL': 'C'}, capture_output=True)
    return binary_path.read_bytes(), source


def package(directory, version, target):
    platform, executable, runtime = TARGETS[target]
    binary, source = compile_payload(directory, version, target)
    prefix = f'token-ashtray-{platform}-x86_64-{version}'
    notices = (f'OFFLINE SYNTHETIC FIXTURE ONLY\nVersion {version}; target {target}.\n'
               'Not Token Ashtray and not a release THIRD-PARTY.md.\n'
               'GCC/MinGW runtime imports are not a production notices audit.\n').encode()
    manifest = {'fixture_only': True, 'version': version, 'target': target,
                'binary_sha256': sha(binary), 'notices_sha256': sha(notices)}
    members = [(f'{prefix}/{executable}', binary, 0o100755),
               (f'{prefix}/THIRD-PARTY.md', notices, 0o100644),
               (f'{prefix}/release-manifest.json', canonical(manifest) + b'\n', 0o100644)]
    payload = zip_bytes(members)
    path = f'stable/{target}/{version}/token-ashtray-{platform}.zip'
    entry = {'length': len(payload), 'hashes': {'sha256': sha(payload)},
             'custom': {'product': 'token-ashtray', 'channel': 'stable', 'version': version,
                        'target': target, 'fixture_only': True, 'purpose': 'update',
                        'artifact_kind': 'portable-zip-v1-fixture',
                        'source_kind': 'synthetic-fixture-c-source',
                        'fixture_source_sha256': sha(source), 'minimum_runtime': runtime,
                        'binary_sha256': sha(binary), 'notices_sha256': sha(notices)}}
    return path, payload, entry


def binding(data):
    return {'version': json.loads(data)['signed']['version'], 'length': len(data),
            'hashes': {'sha256': sha(data)}}


def metadata(targets, root_body, version):
    targets_blob = canonical(sign(targets, root_body['roles']['targets']['keyids'])) + b'\n'
    snapshot = base('snapshot', version)
    snapshot['meta'] = {'targets.json': binding(targets_blob)}
    snapshot_blob = canonical(sign(snapshot, root_body['roles']['snapshot']['keyids'])) + b'\n'
    timestamp = base('timestamp', version)
    timestamp['meta'] = {'snapshot.json': binding(snapshot_blob)}
    return {'targets': targets_blob, 'snapshot': snapshot_blob,
            'timestamp': canonical(sign(timestamp, root_body['roles']['timestamp']['keyids'])) + b'\n'}


def generate(directory):
    if directory.exists():
        raise SystemExit('output directory must not already exist: ' + str(directory))
    directory.mkdir(parents=True)
    roots = [root(i) for i in range(1, 4)]
    write(directory / 'bootstrap/1.root.json', sign(roots[0], root_signers(roots[0])))
    for index in (1, 2):
        write(directory / f'roots/{index + 1}.root.json',
              sign(roots[index], root_signers(roots[index - 1]) + root_signers(roots[index])))
    releases = {}
    for name, version, epoch, sequence in [('n', '0.2.1', 1, 1), ('n-plus-one', '0.2.2', 3, 2)]:
        targets = base('targets', sequence)
        targets['targets'] = {}
        packages = {}
        for target in TARGETS:
            path, payload, entry = package(directory, version, target)
            packages[path] = payload
            targets['targets'][path] = entry
            write(directory / 'releases' / name / 'artifacts' / path, payload)
        blobs = metadata(targets, roots[epoch - 1], sequence)
        for role, data in blobs.items():
            write(directory / 'releases' / name / (role + '.json'), data)
        releases[name] = (targets, packages, blobs)

    vectors = []
    linux, windows = TARGETS.keys()
    original_targets, packages, original_metadata = releases['n-plus-one']
    linux_path = next(path for path in packages if linux in path)
    def add(name, code='verified', **options):
        vector = {'name': name, 'target': linux,
                  'expected': 'ACCEPT' if code == 'verified' else 'REJECT', 'code': code, **options}
        vectors.append(vector)
        return vector

    def changed_targets(name, edit, code, artifact=None, target=linux, artifact_path=linux_path):
        targets = copy.deepcopy(original_targets)
        edit(targets)
        blobs = metadata(targets, roots[2], 2)
        vector = add(name, code, target=target)
        vector['metadata'] = {}
        for role, data in blobs.items():
            path = f'cases/{name}/{role}.json'
            write(directory / path, data)
            vector['metadata'][role] = path
        if artifact is not None:
            path = f'cases/{name}/artifact.zip'
            write(directory / path, artifact)
            vector['artifacts'] = {artifact_path: path}
        return vector

    def custom_edit(field, value):
        return lambda targets: targets['targets'][linux_path]['custom'].__setitem__(field, value)

    for target in TARGETS:
        add('initial-n-' + target, target=target, release='n', roots=[], current='0.2.0')
        add('n-to-n-plus-one-' + target, target=target)
    add('long-offline-two-sequential-roots')
    add('same-version', 'no-upgrade', current='0.2.2')
    add('older-version', 'no-upgrade', current='0.3.0')
    changed_targets('prerelease', custom_edit('version', '0.2.3-rc.1'), 'not-stable-semver')
    changed_targets('build-only-bump', custom_edit('version', '0.2.1+build2'), 'no-upgrade')
    changed_targets('leading-zero-version', custom_edit('version', '0.02.3'), 'not-stable-semver')
    add('unsupported-target', 'unsupported-target', target='aarch64-unknown-linux-gnu')
    changed_targets('wrong-purpose', custom_edit('purpose', 'installer'), 'package-purpose')
    changed_targets('wrong-runtime', custom_edit('minimum_runtime', {'glibc': '9.99'}), 'runtime-constraint')
    changed_targets('wrong-channel', custom_edit('channel', 'beta'), 'product-channel')
    changed_targets('wrong-target', custom_edit('target', windows), 'target-mapping')
    changed_targets('wrong-artifact-kind', custom_edit('artifact_kind', 'installer-elf'), 'package-purpose')
    changed_targets('arbitrary-url', lambda t: t['targets'].__setitem__(
        'https://unapproved.invalid/artifact.zip', t['targets'].pop(linux_path)), 'artifact-path')
    def duplicate_target(targets):
        targets['targets']['duplicate.zip'] = copy.deepcopy(targets['targets'][linux_path])
    changed_targets('duplicate-target-mapping', duplicate_target, 'target-mapping')

    for target in TARGETS:
        artifact_path = next(path for path in packages if target in path)
        original = packages[artifact_path]
        for suffix, payload, code in [('bit-flip', original[:100] + bytes([original[100] ^ 1]) + original[101:], 'artifact-hash'),
                                      ('truncated', original[:-1], 'artifact-length'),
                                      ('appended', original + b'X', 'artifact-length')]:
            name = suffix + '-' + target
            path = f'cases/{name}/artifact.zip'
            write(directory / path, payload)
            add(name, code, target=target, artifacts={artifact_path: path})

        # Reach the artifact cap with correct signatures, length, SHA-256 and
        # internal manifest bindings. Padding the synthetic executable leaves a
        # valid three-member ZIP; its bytes are never executed by verification.
        with zipfile.ZipFile(io.BytesIO(original)) as archive:
            padded_members = [(i.filename, archive.read(i), i.external_attr >> 16)
                              for i in archive.infolist()]
        executable = TARGETS[target][1]
        padded_binary = next(data for name, data, _ in padded_members
                             if name.endswith('/' + executable))
        padded_binary += b'\0' * (LIMIT + 1 - len(padded_binary))
        rebound_members = []
        for member_name, data, mode in padded_members:
            if member_name.endswith('/' + executable):
                data = padded_binary
            elif member_name.endswith('/release-manifest.json'):
                manifest = json.loads(data)
                manifest['binary_sha256'] = sha(padded_binary)
                data = canonical(manifest) + b'\n'
            rebound_members.append((member_name, data, mode))
        oversized = zip_bytes(rebound_members)
        def oversize_binding(targets):
            entry = targets['targets'][artifact_path]
            entry['length'] = len(oversized)
            entry['hashes']['sha256'] = sha(oversized)
            entry['custom']['binary_sha256'] = sha(padded_binary)
        changed_targets('oversize-signed-' + target, oversize_binding,
                        'artifact-size-limit', oversized,
                        target=target, artifact_path=artifact_path)

    def rebound_artifact(name, payload, code):
        def edit(targets):
            entry = targets['targets'][linux_path]
            entry['length'] = len(payload)
            entry['hashes']['sha256'] = sha(payload)
        changed_targets(name, edit, code, payload)
    with zipfile.ZipFile(io.BytesIO(packages[linux_path])) as archive:
        members = [(i.filename, archive.read(i), i.external_attr >> 16) for i in archive.infolist()]
    wrong_manifest = []
    for name, data, mode in members:
        if name.endswith('/release-manifest.json'):
            value = json.loads(data)
            value['binary_sha256'] = '0' * 64
            data = canonical(value)
        wrong_manifest.append((name, data, mode))
    rebound_artifact('manifest-binary-mismatch', zip_bytes(wrong_manifest), 'package-binary-hash')
    rebound_artifact('zip-traversal', zip_bytes(members + [('../escape', b'x', 0o100644)]), 'unsafe-zip-path')
    rebound_artifact('zip-symlink', zip_bytes(members + [('link', b'/tmp/escape', 0o120777)]), 'unsafe-zip-type')
    # Deliberate duplicate member is a required hostile-input vector.
    import warnings
    with warnings.catch_warnings():
        warnings.simplefilter('ignore', UserWarning)
        rebound_artifact('zip-duplicate', zip_bytes(members + [members[0]]), 'duplicate-zip-member')

    for role in ('timestamp', 'snapshot', 'targets'):
        add('rollback-' + role, 'rollback-' + role, floors={role: 3})
        envelope = json.loads(original_metadata[role])
        envelope['signed']['expires'] = '2026-11-20T00:00:00Z'
        data = sign(envelope['signed'], roots[2]['roles'][role]['keyids'])
        path = f'cases/expired-{role}/{role}.json'
        write(directory / path, data)
        add('expired-' + role, 'expired-' + role, metadata={role: path})
    envelope = json.loads(original_metadata['targets'])
    envelope['signed']['version'] = 999
    write(directory / 'cases/modified-metadata/targets.json', envelope)
    add('modified-metadata', 'signature-targets', metadata={'targets': 'cases/modified-metadata/targets.json'})
    write(directory / 'cases/revoked-targets-key/targets.json',
          sign(original_targets, roots[0]['roles']['targets']['keyids']))
    add('revoked-targets-key', 'signature-targets', metadata={'targets': 'cases/revoked-targets-key/targets.json'})
    rogue = key('untrusted-attacker-key')
    write(directory / 'cases/unknown-targets-key/targets.json', sign(original_targets, [rogue]))
    add('unknown-targets-key', 'signature-targets', metadata={'targets': 'cases/unknown-targets-key/targets.json'})
    snapshot = json.loads(original_metadata['snapshot'])['signed']
    snapshot['meta']['targets.json']['hashes']['sha256'] = '0' * 64
    write(directory / 'cases/mix-match/snapshot.json', sign(snapshot, roots[2]['roles']['snapshot']['keyids']))
    snapshot_blob = (directory / 'cases/mix-match/snapshot.json').read_bytes()
    timestamp = json.loads(original_metadata['timestamp'])['signed']
    timestamp['meta']['snapshot.json'] = binding(snapshot_blob)
    write(directory / 'cases/mix-match/timestamp.json', sign(timestamp, roots[2]['roles']['timestamp']['keyids']))
    add('mix-match', 'metadata-hash', metadata={role: f'cases/mix-match/{role}.json' for role in ('timestamp', 'snapshot')})
    add('freeze-expired-clock', 'expired-timestamp', now='2026-11-23T12:00:00Z')
    add('skipped-root', 'root-sequence', roots=['roots/3.root.json'])
    for name, signers in [('root-only-new', root_signers(roots[1])),
                          ('root-only-old', root_signers(roots[0])),
                          ('root-duplicate-signature', [root_signers(roots[0])[0]] * 2 + root_signers(roots[1]))]:
        path = f'cases/{name}/2.root.json'
        write(directory / path, sign(roots[1], signers))
        add(name, 'signature-root', roots=[path, 'roots/3.root.json'])
    expired_root = copy.deepcopy(roots[2])
    expired_root['expires'] = '2026-11-20T00:00:00Z'
    write(directory / 'cases/expired-root/3.root.json',
          sign(expired_root, root_signers(roots[1]) + root_signers(roots[2])))
    add('expired-root', 'expired-root', roots=['roots/2.root.json', 'cases/expired-root/3.root.json'])

    write(directory / 'vectors.json', vectors)
    write(directory / 'PUBLIC-TEST-SEEDS-NEVER-PRODUCTION.json', {'warning': SEED_WARNING, 'keys': SEEDS})
    # Interoperable raw signature inputs let an independent verifier use OpenSSL.
    final_targets = original_targets
    kid = roots[2]['roles']['targets']['keyids'][0]
    write(directory / 'openssl/targets.signed', canonical(final_targets))
    write(directory / 'openssl/targets.sig', KEYS[kid][0].sign(canonical(final_targets)))
    write(directory / 'openssl/targets-public.pem', KEYS[kid][0].public_key().public_bytes(
        serialization.Encoding.PEM, serialization.PublicFormat.SubjectPublicKeyInfo))
    files = {str(path.relative_to(directory)): {'length': path.stat().st_size, 'sha256': sha(path.read_bytes())}
             for path in sorted(directory.rglob('*')) if path.is_file()}
    write(directory / 'byte-manifest.json', {'fixture_only': True, 'files': files})
    print(json.dumps({'fixture_only': True, 'files': len(files) + 1, 'vectors': len(vectors),
                      'positive': sum(v['expected'] == 'ACCEPT' for v in vectors),
                      'negative': sum(v['expected'] == 'REJECT' for v in vectors)}, sort_keys=True))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    generate(parser.parse_args().directory.resolve())
