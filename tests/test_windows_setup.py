"""Validate the single-EXE installer inputs without installing into a user profile."""

import hashlib
import importlib.util
import json
import os
from pathlib import Path, PureWindowsPath
import re
import shutil
import struct
import subprocess
import tempfile
import unittest
from unittest import mock
import warnings
import zipfile


ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("windows_setup", ROOT / "scripts/build_windows_setup.py")
setup = importlib.util.module_from_spec(spec)
spec.loader.exec_module(setup)


class SetupInputs(unittest.TestCase):
    def package(self, root, *, extra=None, missing=None, machine=0x8664, subsystem=2,
                wrong_hash=False):
        binary = bytearray(512)
        binary[:2] = b"MZ"
        struct.pack_into("<I", binary, 0x3C, 0x80)
        binary[0x80:0x84] = b"PE\0\0"
        struct.pack_into("<H", binary, 0x84, machine)
        struct.pack_into("<H", binary, 0x98, 0x20B)
        struct.pack_into("<H", binary, 0x98 + 68, subsystem)
        members = {name: b"fixture notice" for name in setup.PACKAGE_MEMBERS}
        members["install-windows.ps1"] = (ROOT / "install-windows.ps1").read_bytes()
        members["token-ashtray.exe"] = binary
        members["RELEASE-METRICS.json"] = json.dumps({
            "binary_sha256": "bad" if wrong_hash else hashlib.sha256(binary).hexdigest(),
        }).encode()
        if extra:
            members[extra] = b"private canary"
        if missing:
            members.pop(missing)
        archive = root / "fixture.zip"
        with zipfile.ZipFile(archive, "w") as output:
            for name, content in members.items():
                output.writestr(name, content)
        return archive

    def test_exact_package_is_staged_with_legacy_marker_and_notices(self):
        with tempfile.TemporaryDirectory(prefix="setup path ") as directory:
            root = Path(directory)
            stage = root / "stage"
            setup.stage_package(self.package(root), stage)
            self.assertEqual({p.name for p in stage.iterdir()},
                             setup.PACKAGE_MEMBERS | {".token-ashtray-install", ".token-ashtray-setup", "INNO-LICENSE.txt"})
            self.assertEqual((stage / ".token-ashtray-install").read_bytes(),
                             b"Token Ashtray per-user installation\n")
            self.assertIn("Copyright (C) 1997-2025 Jordan Russell", (stage / "INNO-LICENSE.txt").read_text())

    def test_foreign_members_and_missing_notices_are_rejected(self):
        for kwargs in ({"extra": "credential-canary"}, {"extra": "../outside"},
                       {"missing": "THIRD-PARTY.md"}, {"missing": "install-windows.ps1"}):
            with self.subTest(kwargs=kwargs), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                with self.assertRaisesRegex(ValueError, "members"):
                    setup.stage_package(self.package(root, **kwargs), root / "stage")
                self.assertFalse((root / "stage").exists())

    def test_duplicate_members_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = self.package(root)
            with warnings.catch_warnings():
                warnings.simplefilter("ignore", UserWarning)
                with zipfile.ZipFile(archive, "a") as output:
                    output.writestr("LICENSE", b"conflicting notice")
            with self.assertRaisesRegex(ValueError, "members"):
                setup.stage_package(archive, root / "stage")

    def test_hash_mismatch_is_rejected_before_staging(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(ValueError, "SHA-256"):
                setup.stage_package(self.package(root, wrong_hash=True), root / "stage")
            self.assertFalse((root / "stage").exists())

    def test_only_x64_gui_binary_is_accepted(self):
        for kwargs in ({"machine": 0x14C}, {"subsystem": 3}):
            with self.subTest(kwargs=kwargs), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                with self.assertRaisesRegex(ValueError, "x64 GUI"):
                    setup.stage_package(self.package(root, **kwargs), root / "stage")

    def test_legacy_helper_without_read_only_contract_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = self.package(root)
            with zipfile.ZipFile(archive) as source:
                members = {name: source.read(name) for name in source.namelist()}
            members["install-windows.ps1"] = b"legacy helper without ValidateOnly"
            with zipfile.ZipFile(archive, "w") as output:
                for name, content in members.items():
                    output.writestr(name, content)
            with self.assertRaisesRegex(ValueError, "preflight contract"):
                setup.stage_package(archive, root / "stage")
            self.assertFalse((root / "stage").exists())

    def test_package_version_reads_manifest_on_python310_and_newer(self):
        self.assertRegex(setup.package_version(), r"^\d+\.\d+\.\d+")
        with tempfile.TemporaryDirectory() as directory:
            manifest = Path(directory) / "Cargo.toml"
            manifest.write_text(
                '[dependencies]\nversion = "9.9.9"\n\n'
                '[package]\nname = "token-ashtray"\nversion = "1.2.3" # release\n\n'
                '[package.metadata]\nversion = "8.8.8"\n',
                encoding="utf-8",
            )
            self.assertEqual(setup.package_version(manifest), "1.2.3")
            with mock.patch.object(setup, "tomllib", None):
                self.assertEqual(setup.package_version(manifest), "1.2.3")
            missing = Path(directory) / "Missing.toml"
            missing.write_text('[package]\nname = "token-ashtray"\n', encoding="utf-8")
            with mock.patch.object(setup, "tomllib", None):
                with self.assertRaisesRegex(ValueError, "version"):
                    setup.package_version(missing)


# The marker a setup refused by another running setup leaves beside its log.
REFUSED = ".refused"


class UpdateHandoff(unittest.TestCase):
    """The app's updater starts setup with /WAITAPP just before it exits."""

    script = (ROOT / "packaging/windows/setup.iss").read_text(encoding="utf-8")

    def test_setup_waits_for_the_app_guard_before_any_check(self):
        instance = (ROOT / "src/platform/instance.rs").read_text(encoding="utf-8")
        guard = re.search(r'name: r"(Local\\TokenAshtray-\{[0-9A-F-]+\})"', instance).group(1)
        app_id = re.search(r"^AppId=\{(\{[0-9A-F-]+\})$", self.script, re.M).group(1)
        self.assertTrue(guard.endswith(app_id))
        self.assertIn(f"AppGuard = '{guard}';", self.script)
        self.assertIn("AppExitTimeoutSeconds = 60;", self.script)
        wait = self.script[self.script.index("function WaitForAppExit"):]
        self.assertIn("if not HasSwitch(WaitAppSwitch) then\n    Exit;", wait)
        self.assertIn("while CheckForMutexes(AppGuard) and (Tenths < AppExitTimeoutSeconds * 10)", wait)
        self.assertIn("Nothing was changed.", wait)
        prepare = self.script[self.script.index("function PrepareToInstall"):]
        self.assertTrue(prepare.startswith(
            "function PrepareToInstall(var NeedsRestart: Boolean): String;\nbegin\n"
            "  Result := WaitForAppExit;\n  if Result <> '' then\n    Exit;\n"))
        self.assertLess(prepare.index("WaitForAppExit"), prepare.index("RunHelper"))

    def test_only_one_setup_runs_at_a_time(self):
        guard = re.search(r"^  SetupGuard = '(Local\\TokenAshtraySetup-\{[0-9A-F-]+\})';$",
                          self.script, re.M).group(1)
        app_guard = re.search(r"^  AppGuard = '([^']+)';$", self.script, re.M).group(1)
        # Setup must never hold the guard it waits for.
        self.assertNotEqual(guard, app_guard)
        self.assertEqual(guard.rsplit("-", 5)[1:], app_guard.rsplit("-", 5)[1:])
        # One atomic Windows call creates the guard or reports that it exists;
        # a check followed by a separate create would let two setups through.
        self.assertIn("external 'CreateMutexW@kernel32.dll stdcall';", self.script)
        self.assertIn("ERROR_ALREADY_EXISTS = 183;", self.script)
        self.assertNotIn("CheckForMutexes(SetupGuard)", self.script)
        initialize = self.script[self.script.index("function InitializeSetup: Boolean;"):]
        initialize = initialize[:initialize.index("\nend;")]
        self.assertIn("SetupGuardHandle := CreateMutexW(0, 0, SetupGuard);", initialize)
        self.assertIn("DLLGetLastError = ERROR_ALREADY_EXISTS", initialize)
        self.assertIn("MarkRefused;", initialize)
        self.assertIn("Nothing was changed.", initialize)

    def test_a_refused_setup_marks_its_start_for_the_app(self):
        # The app does not count a start that another running setup refused.
        mark = self.script[self.script.index("procedure MarkRefused;"):]
        mark = mark[:mark.index("\nend;")]
        self.assertIn(f"RefusedExtension = '{REFUSED}';", self.script)
        # Read the path, strip any quotes Inno kept, then derive the marker,
        # in this order. A Windows path cannot contain a quote.
        steps = ["LogFile := ExpandConstant('{param:LOG|}');",
                 "StringChangeEx(LogFile, '\"', '', True);",
                 "ChangeFileExt(LogFile, RefusedExtension)"]
        positions = [mark.find(step) for step in steps]
        self.assertNotIn(-1, positions)
        self.assertEqual(positions, sorted(positions))
        # The same steps give one marker whether or not the value kept quotes.
        log = r"C:\Users\QA User\AppData\Local\rammi\token-ashtray\data\updates\apply-0.2.2-2.log"
        marker = log[:-len(".log")] + REFUSED
        for value in (log, '"' + log + '"'):
            with self.subTest(value=value):
                stripped = value.replace('"', '')
                self.assertEqual(str(PureWindowsPath(stripped).with_suffix(REFUSED)), marker)
        store = (ROOT / "src/updates/store.rs").read_text(encoding="utf-8")
        self.assertIn(f'const REFUSED_SUFFIX: &str = "{REFUSED}";', store)

    def test_the_app_passes_the_switch_setup_reads(self):
        self.assertIn("WaitAppSwitch = '/WAITAPP';", self.script)
        windows = (ROOT / "src/updates/windows.rs").read_text(encoding="utf-8")
        self.assertIn('pub const WAIT_APP: &str = "/WAITAPP";', windows)
        # Interactive Setup keeps its own running-app check; no AppMutex prompt.
        self.assertNotIn("AppMutex=", self.script)

    def test_release_builds_never_enable_the_qa_update_feature(self):
        feature = "qa-offline-update"
        manifest = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
        features = manifest[manifest.index("[features]"):].split("\n[", 1)[0]
        self.assertRegex(features, rf"(?m)^{feature} = \[\]$")
        default = re.search(r"(?m)^default = (.*)$", features)
        self.assertTrue(default is None or feature not in default.group(1))
        self.assertIn('required-features = ["qa-offline-update"]', manifest)
        builds = [ROOT / ".github/workflows/check.yml", ROOT / ".github/workflows/release.yml",
                  ROOT / "docs/RELEASE.md", ROOT / "packaging/windows/README.md"]
        for path in builds:
            with self.subTest(path=path.name):
                text = path.read_text(encoding="utf-8")
                self.assertIn("cargo build --release --locked", text)
                self.assertNotIn(feature, text)
                self.assertNotIn("--all-features", text)


@unittest.skipUnless(os.name == "nt" and shutil.which("powershell.exe"), "Native Windows helper checks")
class SetupPreflight(unittest.TestCase):
    def test_checks_preserve_all_files_and_raw_disabled_startup(self):
        # Use the complete helper with disposable APPDATA and isolated HKCU keys.
        # No real application shortcut, autostart entry or provider profile is read.
        harness = r'''
$ErrorActionPreference = 'Stop'
$testRoot = $env:FIXTURE_ROOT
$registry = 'Software\TokenAshtraySetupTest-' + [guid]::NewGuid().ToString('N')
$runPath = $registry + '\Run'
$approvedPath = $registry + '\StartupApproved'
$source = [IO.File]::ReadAllText($env:INSTALL_SCRIPT)
$source = $source.Replace('Software\Microsoft\Windows\CurrentVersion\Run', $runPath)
$source = $source.Replace('Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run', $approvedPath)
$script = Join-Path $testRoot 'install-windows.ps1'
[IO.File]::WriteAllText($script, $source)
$destination = Join-Path $testRoot 'installed app'
$binary = Join-Path $destination 'token-ashtray.exe'
$shortcutPath = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Token Ashtray.lnk'
try {
    & powershell.exe -NoProfile -NonInteractive -File $script -Action Install -InstallDir $destination -ValidateOnly
    if ($LASTEXITCODE -ne 0) { throw 'Clean preflight failed' }
    if (Test-Path -LiteralPath $destination) { throw 'Preflight created the destination' }
    if (Test-Path -LiteralPath $shortcutPath) { throw 'Preflight created the shortcut' }
    [IO.Directory]::CreateDirectory($destination) | Out-Null
    [IO.File]::WriteAllText((Join-Path $destination '.token-ashtray-install'), "Token Ashtray per-user installation`n")
    [IO.File]::WriteAllText($binary, 'old application fixture')
    [IO.File]::WriteAllText((Join-Path $destination 'unrelated.txt'), 'keep')
    [IO.Directory]::CreateDirectory([IO.Path]::GetDirectoryName($shortcutPath)) | Out-Null
    $shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcutPath)
    $shortcut.TargetPath = $binary
    $shortcut.Description = 'Token Ashtray per-user launcher'
    $shortcut.Save()
    $command = '"' + $binary + '" --autostart'
    $disabled = [byte[]](3, 0, 0, 0, 0x58, 0x2F, 0xA4, 0xC7, 0x8F, 0x76, 0xDD, 1)
    $run = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($runPath)
    $run.SetValue('Token Ashtray', $command)
    $run.Close()
    $approved = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($approvedPath)
    $approved.SetValue('Token Ashtray', $disabled, [Microsoft.Win32.RegistryValueKind]::Binary)
    $approved.Close()
    $before = (Get-ChildItem -LiteralPath $testRoot -Recurse -File | Sort-Object FullName | ForEach-Object { $_.FullName + ':' + [Convert]::ToBase64String([IO.File]::ReadAllBytes($_.FullName)) }) -join "`n"
    foreach ($action in @('Install', 'Uninstall')) {
        & powershell.exe -NoProfile -NonInteractive -File $script -Action $action -InstallDir $destination -ValidateOnly
        if ($LASTEXITCODE -ne 0) { throw "$action preflight rejected owned items" }
    }
    $after = (Get-ChildItem -LiteralPath $testRoot -Recurse -File | Sort-Object FullName | ForEach-Object { $_.FullName + ':' + [Convert]::ToBase64String([IO.File]::ReadAllBytes($_.FullName)) }) -join "`n"
    if ($before -ne $after) { throw 'Preflight changed fixture files' }
    $run = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($runPath)
    if ($run.GetValue('Token Ashtray') -ne $command) { throw 'Preflight changed the Run command' }
    $run.Close()
    $approved = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($approvedPath)
    if ([BitConverter]::ToString($approved.GetValue('Token Ashtray')) -ne [BitConverter]::ToString($disabled)) {
        throw 'Preflight changed the raw disabled StartupApproved value'
    }
    $approved.Close()
    $run = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($runPath, $true)
    $run.SetValue('Token Ashtray', 'foreign command')
    $run.Close()
    $ErrorActionPreference = 'Continue'
    & powershell.exe -NoProfile -NonInteractive -File $script -Action Install -InstallDir $destination -ValidateOnly 2>$null
    $ErrorActionPreference = 'Stop'
    if ($LASTEXITCODE -eq 0) { throw 'Preflight accepted a foreign Run command' }
    Write-Output 'Read-only preflight OK'
} finally {
    [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($registry, $false)
}
'''
        with tempfile.TemporaryDirectory(prefix="setup preflight ") as directory:
            root = Path(directory)
            script = root / "test.ps1"
            script.write_text(harness, encoding="utf-8")
            result = subprocess.run([
                "powershell.exe", "-NoProfile", "-NonInteractive", "-File", str(script),
            ], env={**os.environ, "FIXTURE_ROOT": str(root),
                    "APPDATA": str(root / "appdata"), "LOCALAPPDATA": str(root / "local"),
                    "INSTALL_SCRIPT": os.environ.get("TOKEN_ASHTRAY_TEST_INSTALL_SCRIPT", str(ROOT / "install-windows.ps1"))},
                capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("Read-only preflight OK", result.stdout)


if __name__ == "__main__":
    unittest.main()
