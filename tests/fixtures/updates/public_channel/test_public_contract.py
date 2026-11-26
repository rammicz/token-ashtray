"""Public test keys and synthetic bytes; no HTTP, execution or apply acceptance."""
import copy
from pathlib import Path
import tempfile
import unittest

import public_contract as contract


class PublicContract(unittest.TestCase):
    def test_anonymous_origin_and_redirect_boundary(self):
        initial = contract.asset_url('x86_64-unknown-linux-gnu', '0.2.2')
        contract.transport(initial, [])
        contract.transport(initial, ['https://release-assets.githubusercontent.com/example?sig=TEST'])
        for bad in [initial.replace('https:', 'http:'), initial + '?token=TEST',
                    initial.replace('github.com/', 'github.com.evil/'),
                    initial.replace('github.com/', 'user@github.com/'),
                    initial.replace('/v0.2.2/', '/latest/'), initial + '#fragment']:
            with self.subTest(url=bad), self.assertRaises(contract.Reject):
                contract.transport(bad, [])
        for redirects in [['http://release-assets.githubusercontent.com/file'],
                          ['https://example.com/file'],
                          ['https://release-assets.githubusercontent.com:444/file'],
                          ['https://user@release-assets.githubusercontent.com/file'],
                          ['https://release-assets.githubusercontent.com/a',
                           'https://release-assets.githubusercontent.com/b']]:
            with self.subTest(redirects=redirects), self.assertRaises(contract.Reject):
                contract.transport(initial, redirects)

    def test_signed_current_zip_resources_and_tampering(self):
        for target in contract.TARGETS:
            with self.subTest(target=target):
                package, envelope = contract.example(target)
                self.assertGreater(len(package), 2 * 1024 * 1024)
                self.assertEqual(contract.verify(package, envelope, target, '0.2.1'), 'verified-example')
                with self.assertRaisesRegex(contract.Reject, 'artifact-hash'):
                    contract.verify(package[:-1] + bytes([package[-1] ^ 1]), envelope, target, '0.2.1')
                changed = copy.deepcopy(envelope)
                changed['signed']['targets'][0]['version'] = '0.2.3'
                with self.assertRaisesRegex(contract.Reject, 'signature-targets'):
                    contract.verify(package, changed, target, '0.2.1')
                with self.assertRaisesRegex(contract.Reject, 'no-upgrade'):
                    contract.verify(package, envelope, target, '0.2.2')
                other = next(item for item in contract.TARGETS if item != target)
                with self.assertRaisesRegex(contract.Reject, 'target'):
                    contract.verify(package, envelope, other, '0.2.1')

    def test_authenticated_bad_resource_descriptors(self):
        target = next(iter(contract.TARGETS))
        package, envelope = contract.example(target)
        for edit, error in [('omit', 'resource-set'), ('hash', 'member-hash'),
                            ('mode', 'member-mode'), ('url', 'origin-path')]:
            changed = copy.deepcopy(envelope['signed'])
            entry = changed['targets'][0]
            if edit == 'omit':
                entry['members'].pop()
            elif edit == 'hash':
                entry['members'][0]['sha256'] = '0' * 64
            elif edit == 'mode':
                entry['members'][0]['mode'] = 0o777
            else:
                entry['url'] = 'https://example.com/package.zip'
            with self.subTest(edit=edit), self.assertRaisesRegex(contract.Reject, error):
                contract.verify(package, contract.sign(changed), target, '0.2.1')

    def test_offline_generation_is_reproducible(self):
        with tempfile.TemporaryDirectory() as directory:
            first, second = Path(directory) / 'a', Path(directory) / 'b'
            contract.generate(first)
            contract.generate(second)
            a = {p.relative_to(first): p.read_bytes() for p in first.rglob('*') if p.is_file()}
            b = {p.relative_to(second): p.read_bytes() for p in second.rglob('*') if p.is_file()}
            self.assertEqual(a, b)
            self.assertEqual(len(a), 5)


if __name__ == '__main__':
    unittest.main()
