"""OFFLINE FIXTURE ONLY: release-bound signatures, synthetic state, no HTTP/apply.

Reuses historical cryptographic primitives, not its four-role/expiry workflow.
This verifier is not imported by the app or a production signing tool.
"""
import copy
import importlib.util
import json
from pathlib import Path
import re
from urllib.parse import urlsplit

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('release_manifest_legacy_crypto', HERE.parent / 'offline_prototype/verify.py')
legacy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(legacy)
Reject, require, canonical, sha = legacy.Reject, legacy.require, legacy.canonical, legacy.sha

ROOT_SCHEMA = 'release-root-fixture-v1'
MANIFEST_SCHEMA = 'release-manifest-fixture-v1'
BASE = 'https://github.com/rammicz/token-ashtray/releases/'
MANIFEST_NAME = 'token-ashtray-updates.json'
DISCOVERY = BASE + 'latest/download/' + MANIFEST_NAME
CDN = 'release-assets.githubusercontent.com'
MAX_METADATA = 64 * 1024
MAX_ARTIFACT = 128 * 1024 * 1024
MAX_TRANSITIONS = 8
TARGETS = {'x86_64-unknown-linux-gnu': 'linux-setup-elf-v1',
           'x86_64-pc-windows-msvc': 'windows-setup-exe-v1'}


def stable(value):
    require(isinstance(value, str) and len(value) <= 64 and
            re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', value), 'stable-version')
    return tuple(map(int, value.split('.')))


def integer(value):
    return type(value) is int and 0 < value <= 2**53 - 1


def digest(value):
    return isinstance(value, str) and re.fullmatch(r'[0-9a-f]{64}', value)


def exact(value, fields):
    require(isinstance(value, dict) and set(value) == set(fields), 'schema-fields')


def header(value, schema):
    require(value['schema'] == schema and value['fixture_only'] is True and
            value['product'] == 'token-ashtray' and value['repository'] == 'rammicz/token-ashtray'
            and value['channel'] == 'stable', 'schema-product-channel')


def policy(value):
    exact(value, ('schema', 'fixture_only', 'product', 'repository', 'channel', 'version', 'keys', 'roles'))
    header(value, ROOT_SCHEMA)
    require(integer(value['version']), 'root-version')
    keys = value['keys']
    require(isinstance(keys, dict) and 2 <= len(keys) <= 6, 'root-key-count')
    for kid, key in keys.items():
        exact(key, ('keytype', 'scheme', 'keyval'))
        exact(key['keyval'], ('public',))
        require(key['keytype'] == key['scheme'] == 'ed25519' and digest(key['keyval']['public'])
                and kid == sha(canonical(key)), 'root-key')
    exact(value['roles'], ('root', 'release'))
    all_ids = set()
    for name in ('root', 'release'):
        role = value['roles'][name]
        exact(role, ('keyids', 'threshold'))
        ids = role['keyids']
        require(isinstance(ids, list) and all(isinstance(kid, str) for kid in ids)
                and 1 <= len(ids) <= 3 and len(set(ids)) == len(ids)
                and set(ids) <= set(keys) and integer(role['threshold'])
                and role['threshold'] <= len(ids) and not all_ids.intersection(ids), 'role-policy')
        require(name != 'root' or role['threshold'] >= 2, 'root-threshold')
        all_ids.update(ids)
    require(all_ids == set(keys), 'unused-policy-key')


def envelope(value):
    exact(value, ('signed', 'signatures'))
    require(isinstance(value['signatures'], list) and 1 <= len(value['signatures']) <= 6, 'signature-count')
    seen = set()
    for signature in value['signatures']:
        exact(signature, ('keyid', 'sig'))
        require(digest(signature['keyid']) and signature['keyid'] not in seen and
                isinstance(signature['sig'], str) and re.fullmatch(r'[0-9a-f]{128}', signature['sig']), 'signature-shape')
        seen.add(signature['keyid'])


def advance_policy(trusted, transitions):
    policy(trusted)
    require(isinstance(transitions, list) and len(transitions) <= MAX_TRANSITIONS, 'rotation-bound')
    current = copy.deepcopy(trusted)
    previous = 0
    for transition in transitions:
        envelope(transition)
        candidate = transition['signed']
        policy(candidate)
        version = candidate['version']
        require(version > previous, 'root-order')
        previous = version
        if version <= current['version']:
            if version == current['version']:
                require(canonical(candidate) == canonical(current), 'root-equivocation')
            continue  # Older retained records confer no new trust.
        require(version == current['version'] + 1, 'root-sequence')
        legacy.signed_by(transition, current, 'root')
        legacy.signed_by(transition, candidate, 'root')
        current = copy.deepcopy(candidate)
    return current


def artifact_name(target, version):
    require(target in TARGETS, 'target')
    stable(version)
    return 'token-ashtray-setup-' + version + '-' + target + ('.exe' if target == 'x86_64-pc-windows-msvc' else '')


def artifact_url(target, version):
    stable(version)
    return BASE + 'download/v' + version + '/' + artifact_name(target, version)


def manifest_url(version):
    stable(version)
    return BASE + 'download/v' + version + '/' + MANIFEST_NAME


def parts(value):
    require(isinstance(value, str) and len(value) <= 4096 and
            all(0x21 <= ord(char) < 0x7f for char in value), 'url-bound')
    try:
        return urlsplit(value)
    except ValueError as error:
        raise Reject('invalid-url') from error


def asset_redirects(initial, redirects):
    parsed = parts(initial)
    path = parsed.path.split('/')
    require(len(path) == 7 and path[5].startswith('v'), 'asset-origin')
    version = path[5][1:]
    require(initial in [manifest_url(version)] + [artifact_url(t, version) for t in TARGETS], 'asset-origin')
    require(isinstance(redirects, list) and len(redirects) <= 1, 'asset-redirect-count')
    for redirect in redirects:
        dest = parts(redirect)
        require(dest.scheme == 'https' and dest.netloc == CDN and dest.path.startswith('/')
                and not dest.fragment, 'asset-cdn')


def discovery(initial, redirects):
    require(initial == DISCOVERY and isinstance(redirects, list) and 1 <= len(redirects) <= 2, 'latest-discovery')
    tagged = redirects[0]
    parsed = parts(tagged)
    path = parsed.path.split('/')
    require(len(path) == 7 and path[5].startswith('v'), 'manifest-tag')
    version = path[5][1:]
    require(tagged == manifest_url(version), 'manifest-tag')
    asset_redirects(tagged, redirects[1:])
    return 'v' + version


def load(raw):
    require(isinstance(raw, bytes) and 0 < len(raw) <= MAX_METADATA, 'metadata-bound')
    def invalid(_):
        raise Reject('non-finite-json')
    try:
        return json.loads(raw, object_pairs_hook=legacy.unique_object, parse_constant=invalid)
    except (ValueError, UnicodeError, RecursionError) as error:
        raise Reject('invalid-json') from error


def initial_state(root, installed):
    policy(root)
    stable(installed)
    return {'root': copy.deepcopy(root), 'sequence': 0, 'version': installed, 'digest': None, 'pending': False}


def verify(raw, state, target, installed, observed_tag, expected_source):
    """Verify metadata only; return proposed public state for explicit persistence.

    expected_source models an independently reviewed fixture release/build intent;
    real clients cannot infer compiled provenance merely from the signed SHA.
    No filesystem, time, credential, network or installer action occurs here.
    """
    require(target in TARGETS, 'target')
    exact(state, ('root', 'sequence', 'version', 'digest', 'pending'))
    require(type(state['sequence']) is int and 0 <= state['sequence'] <= 2**53 - 1 and
            (state['digest'] is None or digest(state['digest'])) and type(state['pending']) is bool, 'stored-state')
    bundle = load(raw)
    exact(bundle, ('manifest', 'transitions'))
    root = advance_policy(state['root'], bundle['transitions'])
    signed = bundle['manifest']
    envelope(signed)
    body = signed['signed']
    exact(body, ('schema', 'fixture_only', 'product', 'repository', 'channel', 'version', 'tag',
                 'sequence', 'root_version', 'source_sha', 'artifacts'))
    header(body, MANIFEST_SCHEMA)
    require({s['keyid'] for s in signed['signatures']} <= set(root['roles']['release']['keyids']), 'release-key-authority')
    legacy.signed_by(signed, root, 'release')
    version = stable(body['version'])
    require(body['tag'] == observed_tag == 'v' + body['version'], 'version-tag')
    require(integer(body['root_version']) and body['root_version'] == root['version'], 'root-binding')
    require(isinstance(body['source_sha'], str) and re.fullmatch(r'[0-9a-f]{40}', body['source_sha'])
            and body['source_sha'] == expected_source, 'source-binding')
    require(integer(body['sequence']), 'release-sequence')
    exact(body['artifacts'], TARGETS)
    for platform, record in body['artifacts'].items():
        exact(record, ('filename', 'purpose', 'artifact_kind', 'length', 'sha256', 'app_sha256', 'descriptor_sha256'))
        require(record['filename'] == artifact_name(platform, body['version']) and
                record['purpose'] == 'installer' and record['artifact_kind'] == TARGETS[platform]
                and integer(record['length']) and record['length'] <= MAX_ARTIFACT
                and all(digest(record[k]) for k in ('sha256', 'app_sha256', 'descriptor_sha256')), 'artifact-binding')
    require(version >= stable(installed), 'installed-downgrade')
    current_digest = sha(raw)
    if body['sequence'] == state['sequence']:
        require(current_digest == state['digest'] and body['version'] == state['version'], 'same-sequence-change')
        require(state['pending'] or version == stable(installed), 'not-pending')
        status = 'no-update' if version == stable(installed) else 'retry'
    else:
        require(body['sequence'] > state['sequence'] and version >= stable(state['version']), 'release-rollback')
        require(state['sequence'] == 0 or version > stable(state['version']), 'same-version-change')
        status = 'no-update' if version == stable(installed) else 'pending'
    next_state = {'root': root, 'sequence': body['sequence'], 'version': body['version'],
                  'digest': current_digest, 'pending': status != 'no-update'}
    return {'status': status, 'state': next_state, 'artifact': copy.deepcopy(body['artifacts'][target])}


def verify_artifact(data, record):
    require(isinstance(data, bytes) and 0 < len(data) <= MAX_ARTIFACT and len(data) == record['length']
            and sha(data) == record['sha256'], 'artifact-bytes')
