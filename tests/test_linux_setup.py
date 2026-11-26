"""Linux setup artifact integrity and isolated native menu-selector regressions."""
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tests/linux_setup"))
from menu_result import exact_visible_result

MAGIC = b"TOKEN-ASHTRAY-SETUP-v1\n"
NAMES = {"token-ashtray", "token-ashtray-256.png", "LICENSE", "THIRD-PARTY.md",
         "LICENSE-REVIEW.md", "INSTALL-LINUX.md", "RELEASE-METRICS.json"}


@unittest.skipUnless(sys.platform == "linux", "Linux standalone setup artifact")
class LinuxSetupPackage(unittest.TestCase):
    def fixture(self, root):
        app = root / "token-ashtray"
        app.write_bytes(b"\x7fELFapproved app")
        stub = root / "token-ashtray-setup"
        stub.write_bytes(b"\x7fELFsetup stub")
        metrics = root / "metrics.json"
        metrics.write_text(json.dumps({"binary_sha256": hashlib.sha256(app.read_bytes()).hexdigest()}))
        return app, stub, metrics

    def package(self, root, app, stub, metrics):
        out = root / "setup-linux-x86_64"
        run = subprocess.run([sys.executable, "scripts/package_linux_setup.py", str(stub),
                              str(app), str(metrics), str(out), "--source-sha", "a" * 40],
                             cwd=ROOT, capture_output=True, text=True)
        return run, out

    def test_exact_payload_and_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            app, stub, metrics = self.fixture(root)
            run, out = self.package(root, app, stub, metrics)
            self.assertEqual(run.returncode, 0, run.stderr)
            data = out.read_bytes()
            self.assertTrue(data.startswith(stub.read_bytes()))
            self.assertTrue(data.endswith(MAGIC))
            size = struct.unpack("<Q", data[-len(MAGIC)-8:-len(MAGIC)])[0]
            manifest = json.loads(data[-len(MAGIC)-8-size:-len(MAGIC)-8])
            self.assertEqual(manifest["source_sha"], "a" * 40)
            self.assertEqual({m["name"] for m in manifest["members"]}, NAMES)
            offset = len(stub.read_bytes())
            for item in manifest["members"]:
                body = data[offset:offset+item["size"]]
                self.assertEqual(hashlib.sha256(body).hexdigest(), item["sha256"])
                if item["name"] == "token-ashtray":
                    self.assertEqual(body, app.read_bytes())
                offset += item["size"]
            self.assertEqual(offset, len(data)-len(MAGIC)-8-size)
            self.assertTrue(out.stat().st_mode & 0o100)

    def test_invalid_inputs_leave_no_artifact(self):
        for failure in ("empty", "wrong-hash", "not-elf"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                app, stub, metrics = self.fixture(root)
                if failure == "empty":
                    metrics.write_bytes(b"")
                elif failure == "wrong-hash":
                    metrics.write_text('{"binary_sha256":"wrong"}')
                else:
                    app.write_bytes(b"not an executable")
                run, out = self.package(root, app, stub, metrics)
                self.assertNotEqual(run.returncode, 0)
                self.assertFalse(out.exists())


class NativeMenuSelection(unittest.TestCase):
    @staticmethod
    def label(name, bounds=(100, 100, 160, 30), showing=True):
        return {"name": name, "role": "label", "showing": showing, "bounds": list(bounds)}

    def test_app_and_setup_selection_ignores_search_rank(self):
        for names in [("Token Ashtray", "Token Ashtray Setup"),
                      ("Token Ashtray Setup", "Token Ashtray")]:
            labels = [self.label(name, (100 + index * 200, 100, 160, 30))
                      for index, name in enumerate(names)]
            for expected in labels:
                with self.subTest(order=names, target=expected["name"]):
                    self.assertEqual(exact_visible_result(labels, expected["name"], 1280, 1024),
                                     expected)

    def test_missing_hidden_and_unusable_results_are_not_selected(self):
        for labels in [[], [self.label("Token Ashtray Setup")],
                       [self.label("Token Ashtray", showing=False)],
                       [self.label("Token Ashtray", (100, 100, 0, 30))],
                       [self.label("Token Ashtray", (100, 100, 160, 0))],
                       [self.label("Token Ashtray", (-1, 100, 160, 30))],
                       [self.label("Token Ashtray", (1270, 100, 160, 30))]]:
            with self.subTest(labels=labels):
                self.assertIsNone(exact_visible_result(labels, "Token Ashtray", 1280, 1024))

    def test_same_bounds_alias_one_result_but_distinct_bounds_are_ambiguous(self):
        label = self.label("Token Ashtray")
        self.assertEqual(exact_visible_result([label, dict(label)], "Token Ashtray", 1280, 1024),
                         label)
        with self.assertRaisesRegex(AssertionError, "ambiguous exact visible GNOME"):
            exact_visible_result([label, self.label("Token Ashtray", (300, 100, 160, 30))],
                                 "Token Ashtray", 1280, 1024)


if __name__ == "__main__":
    unittest.main()
