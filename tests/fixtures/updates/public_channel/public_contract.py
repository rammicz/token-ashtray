"""OFFLINE EXAMPLES ONLY: public keys, synthetic ZIPs, no network or apply code."""
import argparse
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import stat
import urllib.parse
import zipfile

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[3]
spec = importlib.util.spec_from_file_location('historical_fixture_verify', HERE.parent / 'offline_prototype/verify.py')
legacy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(legacy)
Reject, require, sha, canonical = legacy.Reject, legacy.require, legacy.sha, legacy.canonical
TARGETS = {
    'x86_64-unknown-linux-gnu': ('token-ashtray', 'INSTALL-LINUX.md', 'install-linux.py', 'token-ashtray-256.png'),
    'x86_64-pc-windows-msvc': ('token-ashtray.exe', 'INSTALL-WINDOWS.md', 'install-windows.ps1'),
}
COMMON = ('LICENSE', 'THIRD-PARTY.md', 'LICENSE-REVIEW.md', 'RELEASE-METRICS.json')
LIMIT = 16 * 1024 * 1024  # Example-only bound, not a production release limit.
SEED = hashlib.sha256(b'PUBLIC OFFLINE FIXTURE SEED; NEVER PRODUCTION:public-channel-targets').digest()
KEY = Ed25519PrivateKey.from_private_bytes(SEED)
DESCRIPTION = {'keytype': 'ed25519', 'scheme': 'ed25519', 'keyval': {'public':
    KEY.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw).hex()}}
KEY_ID = sha(canonical(DESCRIPTION))
TRUST = {'roles': {'targets': {'keyids': [KEY_ID], 'threshold': 1}}, 'keys': {KEY_ID: DESCRIPTION}}


def sign(body):
    return {'signed': body, 'signatures': [{'keyid': KEY_ID, 'sig': KEY.sign(canonical(body)).hex()}]}


def asset_url(target, version):
    require(target in TARGETS, 'target')
    legacy.stable(version)
    return (f'https://github.com/rammicz/token-ashtray/releases/download/v{version}/'
            f'token-ashtray-{version}-{target}.zip')


def transport(initial, redirects):
    """Validate supplied observations only; never follows a URL or sends a request."""
    parts = urllib.parse.urlsplit(initial)
    path = parts.path.split('/')
    require(len(path) == 7 and path[5].startswith('v') and not parts.query and not parts.fragment,
            'origin-path')
    version = path[5][1:]
    require(any(initial == asset_url(target, version) for target in TARGETS), 'origin-path')
    require(len(redirects) <= 1, 'redirect-count')
    for url in redirects:
        dest = urllib.parse.urlsplit(url)
        require(dest.scheme == 'https' and dest.netloc == 'release-assets.githubusercontent.com'
                and dest.path.startswith('/') and not dest.fragment, 'redirect-origin')


def verify(package, envelope, target, current):
    legacy.signed_by(envelope, TRUST, 'targets')
    body = envelope['signed']
    legacy.header(body, 'targets', legacy.NOW)
    require(body['schema'] == 'public-packaging-example-v1' and len(body['targets']) == 1, 'schema')
    entry = body['targets'][0]
    require(target in TARGETS and entry['target'] == target, 'target')
    require(entry['fixture_only'] is True and entry['purpose'] == 'packaging-example', 'fixture-purpose')
    require(legacy.stable(entry['version']) > legacy.stable(current), 'no-upgrade')
    transport(entry['url'], [])
    require(entry['url'] == asset_url(target, entry['version']), 'origin-path')
    require(len(package) <= LIMIT, 'artifact-size-limit')
    require(len(package) == entry['length'], 'artifact-length')
    require(sha(package) == entry['sha256'], 'artifact-hash')
    expected = set(COMMON + TARGETS[target])
    descriptors = entry['members']
    require(len(descriptors) == len(expected) and {m['name'] for m in descriptors} == expected, 'resource-set')
    with zipfile.ZipFile(io.BytesIO(package)) as archive:
        infos = archive.infolist()
        require(len(infos) == len(expected) and {i.filename for i in infos} == expected, 'zip-layout')
        require(sum(i.file_size for i in infos) <= LIMIT, 'expanded-limit')
        for member in descriptors:
            info = archive.getinfo(member['name'])
            mode = 0o755 if member['name'] in {'token-ashtray', 'install-linux.py'} else 0o644
            require(stat.S_ISREG(info.external_attr >> 16), 'member-type')
            require(member['mode'] == mode == stat.S_IMODE(info.external_attr >> 16), 'member-mode')
            content = archive.read(info)
            require(len(content) == member['length'], 'member-length')
            require(sha(content) == member['sha256'], 'member-hash')
        metrics = json.loads(archive.read('RELEASE-METRICS.json'))
        require(metrics['binary_sha256'] == sha(archive.read(TARGETS[target][0])), 'metrics-binding')
    return 'verified-example'


def example(target):
    binary = b'OFFLINE SYNTHETIC NONEXECUTABLE FIXTURE ONLY\n' + b'x' * (3 * 1024 * 1024)
    files = {}
    for name in COMMON + TARGETS[target]:
        if name == TARGETS[target][0]:
            content = binary
        elif name == 'RELEASE-METRICS.json':
            content = canonical({'fixture_only': True, 'binary_sha256': sha(binary), 'measurements': 'NONE'})
        else:
            source = {'LICENSE-REVIEW.md': ROOT / 'docs/LICENSE-REVIEW.md',
                      'token-ashtray-256.png': ROOT / 'assets/application/token-ashtray-256.png'}.get(name, ROOT / name)
            content = source.read_bytes()
        files[name] = content
    output = io.BytesIO()
    members = []
    with zipfile.ZipFile(output, 'w', compression=zipfile.ZIP_STORED) as archive:
        for name, content in sorted(files.items()):
            mode = 0o755 if name in {'token-ashtray', 'install-linux.py'} else 0o644
            info = zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0))
            info.create_system = 3
            info.external_attr = (stat.S_IFREG | mode) << 16
            archive.writestr(info, content)
            members.append({'name': name, 'length': len(content), 'sha256': sha(content), 'mode': mode})
    package = output.getvalue()
    entry = {'fixture_only': True, 'purpose': 'packaging-example', 'version': '0.2.2',
             'target': target, 'url': asset_url(target, '0.2.2'), 'length': len(package),
             'sha256': sha(package), 'members': members}
    body = {'_type': 'targets', 'spec_version': '1.0.0', 'fixture_only': True, 'version': 1,
            'expires': '2027-02-19T12:00:00Z', 'schema': 'public-packaging-example-v1', 'targets': [entry]}
    return package, sign(body)


def generate(destination):
    destination.mkdir(parents=True, exist_ok=False)
    hashes = {}
    for target in TARGETS:
        package, envelope = example(target)
        require(verify(package, envelope, target, '0.2.1') == 'verified-example', 'generation')
        for name, content in [(f'{target}.zip', package), (f'{target}.targets.json', canonical(envelope) + b'\n')]:
            (destination / name).write_bytes(content)
            hashes[name] = {'length': len(content), 'sha256': sha(content)}
    (destination / 'byte-manifest.json').write_bytes(canonical({'fixture_only': True, 'files': hashes}) + b'\n')
    return hashes


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('destination', type=Path)
    print(json.dumps(generate(parser.parse_args().destination), indent=2, sort_keys=True))
