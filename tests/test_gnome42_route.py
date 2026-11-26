"""Distribution behavior of the separate, pinned GNOME route."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('gnome_route', ROOT/'integrations/gnome42/build.py')
route = importlib.util.module_from_spec(spec)
spec.loader.exec_module(route)

class DistributionTests(unittest.TestCase):
    def test_reproducible_source_distribution(self):
        with tempfile.TemporaryDirectory() as tmp:
            first, second = Path(tmp)/'first.zip', Path(tmp)/'second.zip'
            route.build(Path('/'),first)
            route.build(Path('/'),second)
            self.assertEqual(first.read_bytes(),second.read_bytes())
            with zipfile.ZipFile(first) as archive:
                metadata = json.loads(archive.read('metadata.json'))
                self.assertEqual(metadata['shell-version'],['42'])
                self.assertEqual(metadata['uuid'],route.UUID)
                self.assertIn(b'GNU GENERAL PUBLIC LICENSE',archive.read('COPYING'))
                self.assertIn(b'Giovanni Campagna',archive.read('COPYRIGHT'))
                self.assertIn('schemas/org.gnome.shell.extensions.appindicator.gschema.xml',archive.namelist())
                self.assertIn('schemas/gschemas.compiled',archive.namelist())
                self.assertIn('interfaces-xml/StatusNotifierItem.xml',archive.namelist())
                self.assertFalse(any(name.endswith(('.rs','.exe')) for name in archive.namelist()))

    def test_changed_source_refused_before_distribution(self):
        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp)/'source'
            inputs = json.loads((route.ROOT/'inputs.json').read_text())
            for relative in inputs:
                target = source/relative
                target.parent.mkdir(parents=True,exist_ok=True)
                target.write_bytes((Path('/')/relative).read_bytes())
            (source/next(iter(inputs))).write_bytes(b'unsupported version')
            output = Path(tmp)/'output.zip'
            with self.assertRaisesRegex(ValueError,'Unsupported input'):
                route.build(source,output)
            self.assertFalse(output.exists())

if __name__ == '__main__':
    unittest.main()
