"""Exercise release startup validation with disposable processes, never CLI logins."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

from test_linux_setup import LinuxSetupPackage, NativeMenuSelection  # noqa: F401
from test_install_resources import (  # noqa: F401
    LinuxInstall, PackageResources, PowerShellInstall, ReleaseBuildFlags,
)
from test_public_export import CommandLine, Export, GitAttribution, Scan  # noqa: F401
from test_windows_setup import SetupInputs, SetupPreflight, UpdateHandoff  # noqa: F401


@unittest.skipUnless(os.name == "posix", "Disposable executable fixtures use POSIX scripts")
class StartupValidation(unittest.TestCase):
    def sample(self, program):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "fixture"
            binary.write_text("#!/usr/bin/env python3\n" + program)
            binary.chmod(0o700)
            result = subprocess.run(
                ["python3", "scripts/release_metrics.py", str(binary), "--seconds", "0.2",
                 "--output", str(root / "metrics.json")],
                env={**os.environ, "CI": "true"}, capture_output=True, text=True, check=False,
            )
            return result.returncode, json.loads((root / "metrics.json").read_text())

    def test_crash_cannot_pass_release_validation(self):
        code, report = self.sample("raise SystemExit(7)\n")
        self.assertNotEqual(code, 0)
        self.assertEqual(report["startup_exit_code"], 7)
        self.assertIn("status 7", report["measurement_note"])

    def test_running_process_can_be_measured(self):
        code, report = self.sample("import time\ntime.sleep(30)\n")
        self.assertEqual(code, 0)
        self.assertGreater(report["idle_rss_bytes"], 0)
        self.assertIsNone(report["startup_exit_code"])


if __name__ == "__main__":
    unittest.main()
