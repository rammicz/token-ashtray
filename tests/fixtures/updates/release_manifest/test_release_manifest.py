"""Public test keys and inert bytes: no network, installer or production signer."""
import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

import release_manifest as profile


def key(label):
    seed = hashlib.sha256(('PUBLIC RELEASE MANIFEST TEST KEY; NEVER PRODUCTION:' + label).encode()).digest()
    private = Ed25519PrivateKey.from_private_bytes(seed)
    public = {'keytype': 'ed25519', 'scheme': 'ed25519', 'keyval': {'public':
        private.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw).hex()}}
    return profile.sha(profile.canonical(public)), public, private


KEYS = {name: key(name) for name in ('old', 'new', 'third', 'root-a', 'root-b', 'root-c', 'root-d', 'root-e', 'root-f')}
SOURCE = 'a' * 40


def policy(version, names, threshold=1, roots=('root-a', 'root-b', 'root-c')):
    keys = {KEYS[name][0]: KEYS[name][1] for name in (*roots, *names)}
    return {'schema': profile.ROOT_SCHEMA, 'fixture_only': True, 'product': 'token-ashtray',
            'repository': 'rammicz/token-ashtray', 'channel': 'stable', 'version': version,
            'keys': keys, 'roles': {
                'root': {'keyids': [KEYS[name][0] for name in roots], 'threshold': 2},
                'release': {'keyids': [KEYS[name][0] for name in names], 'threshold': threshold}}}


def sign(body, names):
    return {'signed': body, 'signatures': [{'keyid': KEYS[name][0],
        'sig': KEYS[name][2].sign(profile.canonical(body)).hex()} for name in names]}


def example(version='0.2.2', sequence=2, epoch=1, names=('old',), transitions=()):
    artifacts = {target: ('SYNTHETIC NONEXECUTABLE ' + target + ' ' + version).encode()
                 for target in profile.TARGETS}
    rows = {target: {'filename': profile.artifact_name(target, version), 'purpose': 'installer',
                    'artifact_kind': kind, 'length': len(artifacts[target]),
                    'sha256': profile.sha(artifacts[target]), 'app_sha256': 'b' * 64,
                    'descriptor_sha256': 'c' * 64} for target, kind in profile.TARGETS.items()}
    body = {'schema': profile.MANIFEST_SCHEMA, 'fixture_only': True, 'product': 'token-ashtray',
            'repository': 'rammicz/token-ashtray', 'channel': 'stable', 'version': version,
            'tag': 'v' + version, 'sequence': sequence, 'root_version': epoch,
            'source_sha': SOURCE, 'artifacts': rows}
    return {'manifest': sign(body, names), 'transitions': list(transitions)}, artifacts


class ReleaseManifest(unittest.TestCase):
    def setUp(self):
        self.root = policy(1, ('old',))
        self.state = profile.initial_state(self.root, '0.2.1')

    def verify(self, bundle, target=None, state=None, installed='0.2.1', source=SOURCE, tag='v0.2.2'):
        return profile.verify(profile.canonical(bundle), state or self.state,
                              target or next(iter(profile.TARGETS)), installed, tag, source)

    def test_both_platforms_signed_binding_and_exact_pending_retry(self):
        bundle, artifacts = example()
        self.assertEqual(profile.canonical(bundle), profile.canonical(example()[0]))
        for target, payload in artifacts.items():
            with self.subTest(target=target):
                result = self.verify(bundle, target)
                self.assertEqual(result['status'], 'pending')
                profile.verify_artifact(payload, result['artifact'])
                restored = json.loads(json.dumps(result['state']))
                retry = self.verify(bundle, target, state=restored)
                self.assertEqual(retry['status'], 'retry')
                self.assertEqual(retry['state'], restored)
                with self.assertRaises(profile.Reject):
                    profile.verify_artifact(payload + b'changed', result['artifact'])

    def test_signature_mutation_and_signed_identity_errors(self):
        bundle, _ = example()
        changed = copy.deepcopy(bundle)
        changed['manifest']['signed']['source_sha'] = 'd' * 40
        with self.assertRaises(profile.Reject):
            self.verify(changed)
        for field, value in (('version', '0.2.3'), ('version', '0.2.2-beta'), ('tag', 'v0.2.3'),
                             ('source_sha', 'd' * 40), ('channel', 'beta'), ('root_version', 2), ('root_version', True)):
            body = copy.deepcopy(bundle['manifest']['signed'])
            body[field] = value
            with self.subTest(field=field, value=value), self.assertRaises(profile.Reject):
                self.verify({'manifest': sign(body, ('old',)), 'transitions': []})
        with self.assertRaises(profile.Reject):
            self.verify(bundle, target='aarch64-unknown-linux-gnu')
        for edit in ('filename', 'kind', 'purpose', 'target'):
            body = copy.deepcopy(bundle['manifest']['signed'])
            target = next(iter(profile.TARGETS))
            if edit == 'filename': body['artifacts'][target]['filename'] = '../evil'
            elif edit == 'kind': body['artifacts'][target]['artifact_kind'] = 'update-zip'
            elif edit == 'purpose': body['artifacts'][target]['purpose'] = 'update'
            else: body['artifacts']['foreign'] = body['artifacts'].pop(target)
            with self.subTest(edit=edit), self.assertRaises(profile.Reject):
                self.verify({'manifest': sign(body, ('old',)), 'transitions': []})

    def test_same_sequence_or_version_cannot_change_digest(self):
        bundle, _ = example()
        state = self.verify(bundle)['state']
        for version, sequence in (('0.2.3', 1), ('0.2.3', 2), ('0.2.2', 3), ('0.2.1', 3)):
            other, _ = example(version, sequence)
            with self.subTest(version=version, sequence=sequence), self.assertRaises(profile.Reject):
                self.verify(other, state=state, tag='v' + version)
        raw = profile.canonical(bundle) + b'\n'
        with self.assertRaises(profile.Reject):
            profile.verify(raw, state, next(iter(profile.TARGETS)), '0.2.1', 'v0.2.2', SOURCE)

    def test_installed_floor_and_known_equal_no_update(self):
        bundle, _ = example()
        with self.assertRaises(profile.Reject):
            self.verify(bundle, installed='0.2.3')
        result = self.verify(bundle, installed='0.2.2')
        self.assertEqual(result['status'], 'no-update')
        again = self.verify(bundle, state=result['state'], installed='0.2.2')
        self.assertEqual(again['status'], 'no-update')

    def test_sequential_dual_threshold_rotation_and_persisted_root_floor(self):
        second = policy(2, ('new',))
        third = policy(3, ('third',), roots=('root-d', 'root-e', 'root-f'))
        chain = [sign(second, ('root-a', 'root-b')),
                 sign(third, ('root-a', 'root-b', 'root-d', 'root-e'))]
        bundle, _ = example(epoch=3, names=('third',), transitions=chain)
        state = self.verify(bundle)['state']
        self.assertEqual(state['root']['version'], 3)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'public-test-state.json'
            path.write_text(json.dumps(state))
            state = json.loads(path.read_text())
        old, _ = example('0.2.3', 3)
        with self.assertRaises(profile.Reject):
            self.verify(old, state=state, tag='v0.2.3')
        self.assertEqual(self.verify(bundle, state=state)['status'], 'retry')

    def test_rotation_rejects_skips_one_side_duplicates_and_excess(self):
        second = policy(2, ('new',), roots=('root-d', 'root-e', 'root-f'))
        valid = sign(second, ('root-a', 'root-b', 'root-d', 'root-e'))
        for chain in ([sign(policy(3, ('new',)), ('root-a', 'root-b'))],
                      [sign(second, ('root-a', 'root-b'))],
                      [sign(second, ('root-d', 'root-e'))],
                      [sign(second, ('root-a', 'root-a', 'root-d', 'root-e'))],
                      [sign(second, ('old', 'new'))],
                      [valid] * (profile.MAX_TRANSITIONS + 1)):
            bundle, _ = example(epoch=2, names=('new',), transitions=chain)
            with self.subTest(chain_length=len(chain)), self.assertRaises(profile.Reject):
                self.verify(bundle)

    def test_release_keys_cannot_rotate_trust_and_root_keys_cannot_sign_release(self):
        second = policy(2, ('new',))
        with self.assertRaises(profile.Reject):
            profile.advance_policy(self.root, [sign(second, ('old', 'new'))])
        bundle, _ = example(names=('root-a', 'root-b'))
        with self.assertRaises(profile.Reject):
            self.verify(bundle)
        advanced = profile.advance_policy(self.root, [sign(second, ('root-a', 'root-b'))])
        persisted = json.loads(json.dumps(self.state))
        persisted['root'] = advanced
        old, _ = example()
        with self.assertRaises(profile.Reject):
            self.verify(old, state=persisted)
        self.assertEqual(persisted['root']['version'], 2)

    def test_exact_latest_tag_and_asset_redirect_graph(self):
        tagged = profile.manifest_url('0.2.2')
        cdn = 'https://release-assets.githubusercontent.com/opaque?sig=TRANSPORT_ONLY'
        self.assertEqual(profile.discovery(profile.DISCOVERY, [tagged]), 'v0.2.2')
        self.assertEqual(profile.discovery(profile.DISCOVERY, [tagged, cdn]), 'v0.2.2')
        self.assertEqual(profile.asset_redirects(profile.artifact_url(next(iter(profile.TARGETS)), '0.2.2'), []), None)
        for initial, hops in ((profile.DISCOVERY + '?token=x', [tagged]),
                              (profile.DISCOVERY.replace('rammicz/', 'other/'), [tagged]),
                              (profile.DISCOVERY, []), (profile.DISCOVERY, [cdn]),
                              (profile.DISCOVERY, [tagged.replace('/v0.2.2/', '/latest/')]),
                              (profile.DISCOVERY, [tagged + '?x=y']),
                              (profile.DISCOVERY, [tagged, cdn, cdn]),
                              (profile.DISCOVERY, [tagged, 'https://evil.test/a']),
                              (profile.DISCOVERY, [tagged, cdn.replace('https:', 'http:')]),
                              (profile.DISCOVERY, [tagged, cdn.replace('.com/', '.com:444/')]),
                              (profile.DISCOVERY, [tagged, 'https://[invalid']),
                              (profile.DISCOVERY, [tagged, cdn + '\n']),
                              (profile.DISCOVERY, [tagged, cdn + ' ']),
                              (profile.DISCOVERY, [tagged.replace('/rammicz/', '/foreign/')]),
                              (profile.DISCOVERY, [tagged, cdn + '#fragment']),
                              (profile.DISCOVERY, [tagged, cdn.replace('https://', 'https://user@')])):
            with self.subTest(initial=initial, hops=hops), self.assertRaises(profile.Reject):
                profile.discovery(initial, hops)
        with self.assertRaises(profile.Reject):
            profile.asset_redirects('https://evil.test/file', [])
        with self.assertRaises(profile.Reject):
            profile.asset_redirects(profile.artifact_url(next(iter(profile.TARGETS)), '0.2.2'), [tagged])

    def test_bounds_and_duplicate_json_rejection(self):
        bundle, _ = example()
        for raw in (b'x' * (profile.MAX_METADATA + 1), b'{"manifest":{},"manifest":{}}', b'NaN'):
            with self.subTest(raw_length=len(raw)), self.assertRaises(profile.Reject):
                profile.verify(raw, self.state, next(iter(profile.TARGETS)), '0.2.1', 'v0.2.2', SOURCE)
        body = copy.deepcopy(bundle['manifest']['signed'])
        body['artifacts'][next(iter(profile.TARGETS))]['length'] = profile.MAX_ARTIFACT + 1
        with self.assertRaises(profile.Reject):
            self.verify({'manifest': sign(body, ('old',)), 'transitions': []})

    def test_old_valid_upgrade_is_accepted_without_freshness_oracle(self):
        # A newer signed release exists, but discovery withholds it. Acceptance is
        # deliberate evidence of the unbounded freeze limitation, NOT a freshness pass.
        old, _ = example('0.2.2', 2)
        newer, _ = example('0.2.3', 3)
        self.assertNotEqual(profile.sha(profile.canonical(old)), profile.sha(profile.canonical(newer)))
        self.assertEqual(self.verify(old)['status'], 'pending')
        self.assertNotIn('expires', old['manifest']['signed'])
        self.assertEqual(self.verify(old, state=self.verify(old)['state'])['status'], 'retry')


if __name__ == '__main__':
    unittest.main()
