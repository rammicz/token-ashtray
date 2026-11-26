"""Offline prototype behavior checks; no app/release/account acceptance."""
import importlib.util
import hashlib
import json
import os
from pathlib import Path
import unittest

HERE = Path(__file__).resolve().parent


def fixture_directory():
    value = os.environ.get('TOKEN_ASHTRAY_FIXTURE_DIRECTORY')
    if not value:
        raise RuntimeError('Set TOKEN_ASHTRAY_FIXTURE_DIRECTORY to freshly generated offline fixtures')
    return Path(value).resolve()


class FixtureContract(unittest.TestCase):
    def test_oversized_artifacts_are_authentically_bound(self):
        root = fixture_directory()
        vectors = {v['name']: v for v in json.loads((root / 'vectors.json').read_text())}
        for target in ('x86_64-unknown-linux-gnu', 'x86_64-pc-windows-msvc'):
            name = 'oversize-signed-' + target
            with self.subTest(target=target):
                self.assertIn(name, vectors, 'required signed oversized-artifact vector absent')
                vector = vectors[name]
                self.assertEqual(vector['code'], 'artifact-size-limit')
                self.assertEqual(vector['expected'], 'REJECT')
                targets = json.loads((root / vector['metadata']['targets']).read_text())['signed']['targets']
                self.assertEqual(len(vector['artifacts']), 1)
                path, override = next(iter(vector['artifacts'].items()))
                payload = (root / override).read_bytes()
                self.assertGreater(len(payload), 2 * 1024 * 1024)
                self.assertEqual(targets[path]['length'], len(payload))
                self.assertEqual(targets[path]['hashes']['sha256'], hashlib.sha256(payload).hexdigest())

    def test_required_signed_payloads_exist(self):
        root = fixture_directory()
        self.assertTrue((root / 'bootstrap/1.root.json').is_file(),
                        'offline signed fixture root/payloads have not been generated')
        for version in ('n', 'n-plus-one'):
            for target in ('x86_64-unknown-linux-gnu', 'x86_64-pc-windows-msvc'):
                self.assertEqual(len(list((root / 'releases' / version / 'artifacts').glob(
                    f'stable/{target}/*/*.zip'))), 1)

    def test_expected_results(self):
        spec = importlib.util.spec_from_file_location('fixture_verify', HERE / 'verify.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        root = fixture_directory()
        vectors = json.loads((root / 'vectors.json').read_text())
        self.assertGreaterEqual(len(vectors), 30)
        for vector in vectors:
            with self.subTest(vector=vector['name']):
                result = module.run_vector(root, vector)
                self.assertEqual(result['outcome'], vector['expected'])
                self.assertEqual(result['code'], vector['code'])


if __name__ == '__main__':
    unittest.main()
