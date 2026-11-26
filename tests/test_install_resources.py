"""Exercise packaged installation resources with disposable files and profiles."""

import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
import zipfile


ROOT = Path(__file__).resolve().parents[1]
PACKAGER = ROOT / "scripts/package_release.py"
LINUX_HELPER = ROOT / "install-linux.py"
PWSH = os.environ.get("TOKEN_ASHTRAY_TEST_PWSH") or shutil.which("pwsh")
sys.path.insert(0, str(ROOT / "scripts"))
import build_windows_setup  # noqa: E402
import release_build  # noqa: E402
BS = chr(92)
# Assembled at runtime so the source tree itself carries no host-path literals.
WINDOWS_REGISTRY = BS.join(["C:", "Users", "builder", ".cargo", "registry", "src",
                            "index.crates.io-1949cf8c6b5b557f", "tokio-1.48.0", "src", "lib.rs"])
UNIX_REGISTRY = "/".join(["", "home", "builder", ".cargo", "registry", "src",
                          "index.crates.io-1949cf8c6b5b557f", "tokio-1.48.0", "src", "lib.rs"])
LEAKY_EXECUTABLES = {
    "windows-ascii": ("windows", b"panic at " + WINDOWS_REGISTRY.encode() + b":12:5\0"),
    "unix-ascii": ("linux", b"panic at " + UNIX_REGISTRY.encode() + b":12:5\0"),
    "windows-utf16": ("windows", b"src/lib.rs\0" + WINDOWS_REGISTRY.encode("utf-16-le") + b"\0\0"),
    # Rust packs string literals back to back, so a path often follows a letter directly.
    "windows-profile-glued": ("windows",
                              b"on a `None` value" + BS.join(["C:", "Users", "builder", "src", "main.rs"]).encode()),
    "windows-lowercase-glued": ("windows", b"value" + BS.join(["c:", "users", "builder", "src", "main.rs"]).encode()),
    "ci-checkout-glued": ("windows", b"value" + BS.join(["D:", "a", "app", "src", "main.rs"]).encode()),
    # Packaged on another host, so only the generic home pattern can see a home with a space.
    "unix-home-with-space": ("linux", b"panic at /home/john smith/work/app/src/main.rs\0"),
    "unix-home-with-space-registry": ("linux", b"/home/john smith/.cargo/registry/src/x/lib.rs\0"),
}
# Toolchain, remapped and relative vendor paths are what a remapped build legitimately keeps.
CLEAN_EXECUTABLE = b"\0".join([
    b"/rustc/58cf8f9c35b355199727627d07ae34a4e2846d67/library/std/src/panicking.rs",
    b"vendor/eframe/src/lib.rs", b"vendor" + BS.encode() + b"winit" + BS.encode() + b"src",
    b"/cargo/registry/src/index.crates.io-0/foo/src/lib.rs",
    ("/cargo" + BS + "registry" + BS + "src").encode("utf-16-le"),
    b"/token-ashtray/vendor/winit/src/lib.rs", b"/target/release/build/glutin_wgl_sys-0/out/bindings.rs",
    b"https://github.com/rammicz/token-ashtray", b"valuehttps://github.com/rammicz/token-ashtray/releases",
])


class PackageResources(unittest.TestCase):
    def package(self, root, platform, executable=b"fixture executable"):
        for name in ("LICENSE", "THIRD-PARTY.md", "INSTALL-LINUX.md", "INSTALL-WINDOWS.md",
                     "install-linux.py", "install-windows.ps1"):
            source = ROOT / name
            if source.exists():
                shutil.copy2(source, root / name)
        (root / "docs").mkdir(exist_ok=True)
        shutil.copy2(ROOT / "docs/LICENSE-REVIEW.md", root / "docs/LICENSE-REVIEW.md")
        (root / "assets/application").mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / "assets/application/token-ashtray-256.png",
                     root / "assets/application/token-ashtray-256.png")
        binary = root / ("token-ashtray.exe" if platform == "windows" else "token-ashtray")
        binary.write_bytes(executable)
        # Packaging must supply target modes even when the source is not executable.
        binary.chmod(0o644)
        (root / "install-linux.py").chmod(0o644)
        # Nor should executable source metadata leak onto packaged documentation.
        (root / "LICENSE").chmod(0o755)
        metrics = root / "metrics.json"
        metrics.write_text("{}")
        output = root / "fixture.zip"
        result = subprocess.run(
            [sys.executable, str(PACKAGER), str(binary), str(metrics), str(output)],
            cwd=root, capture_output=True, text=True,
        )
        return result, output

    def test_exact_platform_resources_and_modes(self):
        for platform in ("linux", "windows"):
            with self.subTest(platform=platform), tempfile.TemporaryDirectory(prefix="package path ") as directory:
                root = Path(directory)
                (root / "credential-canary").write_text("secret")
                result, output = self.package(root, platform)
                self.assertEqual(result.returncode, 0, result.stderr)
                common = {"LICENSE", "THIRD-PARTY.md", "LICENSE-REVIEW.md", "RELEASE-METRICS.json"}
                platform_files = ({"token-ashtray", "INSTALL-LINUX.md", "install-linux.py", "token-ashtray-256.png"}
                                  if platform == "linux" else {"token-ashtray.exe", "INSTALL-WINDOWS.md", "install-windows.ps1"})
                with zipfile.ZipFile(output) as archive:
                    self.assertEqual(set(archive.namelist()), common | platform_files)
                    self.assertNotIn("credential-canary", archive.namelist())
                    self.assertEqual(archive.read("token-ashtray" if platform == "linux" else "token-ashtray.exe"),
                                     b"fixture executable")
                    executables = {"token-ashtray", "install-linux.py"} if platform == "linux" else set()
                    for member in archive.infolist():
                        with self.subTest(member=member.filename):
                            self.assertEqual(member.create_system, 3)
                            mode = member.external_attr >> 16
                            self.assertTrue(stat.S_ISREG(mode))
                            self.assertEqual(stat.S_IMODE(mode),
                                             0o755 if member.filename in executables else 0o644)
                if platform == "linux" and os.name == "posix" and shutil.which("unzip"):
                    extracted = root / "extracted path"
                    extracted.mkdir()
                    unzip = subprocess.run(["unzip", "-q", str(output), "-d", str(extracted)],
                                           capture_output=True, text=True)
                    self.assertEqual(unzip.returncode, 0, unzip.stderr)
                    self.assertTrue(os.access(extracted / "token-ashtray", os.X_OK))
                    self.assertTrue(os.access(extracted / "install-linux.py", os.X_OK))
                    for name in (common | platform_files) - executables:
                        self.assertFalse(os.access(extracted / name, os.X_OK), name)

    def test_missing_platform_resource_rejected_before_archive(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            result, output = self.package(root, "linux")
            self.assertEqual(result.returncode, 0, result.stderr)
            (root / "INSTALL-LINUX.md").write_bytes(b"")
            output.unlink()
            binary = root / "token-ashtray"
            metrics = root / "metrics.json"
            result = subprocess.run([sys.executable, str(PACKAGER), str(binary), str(metrics), str(output)],
                                    cwd=root, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("INSTALL-LINUX.md", result.stderr + result.stdout)
            self.assertFalse(output.exists())

    def assert_redacted_refusal(self, result, output):
        self.assertNotEqual(result.returncode, 0)
        report = result.stdout + result.stderr
        self.assertIn("build-host path", report)
        self.assertRegex(report, r"x[1-9]")
        self.assertNotIn("builder", report)
        self.assertNotIn("john", report)
        self.assertFalse(output.exists())

    def test_build_host_paths_in_executable_are_rejected_before_packaging(self):
        for case, (platform, executable) in LEAKY_EXECUTABLES.items():
            with self.subTest(case=case), tempfile.TemporaryDirectory() as directory:
                result, output = self.package(Path(directory), platform, executable)
                self.assert_redacted_refusal(result, output)

    def test_remapped_toolchain_and_vendor_paths_package_normally(self):
        for platform in ("linux", "windows"):
            with self.subTest(platform=platform), tempfile.TemporaryDirectory() as directory:
                result, output = self.package(Path(directory), platform, CLEAN_EXECUTABLE)
                self.assertEqual(result.returncode, 0, result.stderr)
                with zipfile.ZipFile(output) as archive:
                    name = "token-ashtray" if platform == "linux" else "token-ashtray.exe"
                    self.assertEqual(archive.read(name), CLEAN_EXECUTABLE)

    def test_linux_setup_refuses_build_host_paths_before_writing(self):
        for leaky in ("setup", "app"):
            with self.subTest(leaky=leaky), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                bodies = {"setup": b"\x7fELF stub ", "app": b"\x7fELF app "}
                bodies[leaky] += UNIX_REGISTRY.encode()
                stub, app = root / "token-ashtray-setup", root / "token-ashtray"
                stub.write_bytes(bodies["setup"])
                app.write_bytes(bodies["app"])
                metrics = root / "metrics.json"
                metrics.write_text(json.dumps({"binary_sha256": hashlib.sha256(bodies["app"]).hexdigest()}))
                output = root / "setup-linux-x86_64"
                result = subprocess.run(
                    [sys.executable, str(ROOT / "scripts/package_linux_setup.py"), str(stub), str(app),
                     str(metrics), str(output), "--source-sha", "a" * 40],
                    cwd=ROOT, capture_output=True, text=True)
                self.assert_redacted_refusal(result, output)
                self.assertEqual(list(root.glob(".setup-*")), [])

    def test_windows_setup_staging_refuses_build_host_paths(self):
        binary = bytearray(512)
        binary[:2] = b"MZ"
        struct.pack_into("<I", binary, 0x3C, 0x80)
        binary[0x80:0x84] = b"PE\0\0"
        struct.pack_into("<H", binary, 0x84, 0x8664)
        struct.pack_into("<H", binary, 0x98, 0x20B)
        struct.pack_into("<H", binary, 0x98 + 68, 2)
        binary = bytes(binary) + WINDOWS_REGISTRY.encode("utf-16-le")
        members = {name: b"fixture notice" for name in build_windows_setup.PACKAGE_MEMBERS}
        members["install-windows.ps1"] = (ROOT / "install-windows.ps1").read_bytes()
        members["token-ashtray.exe"] = binary
        members["RELEASE-METRICS.json"] = json.dumps({"binary_sha256": hashlib.sha256(binary).hexdigest()}).encode()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / "fixture.zip"
            with zipfile.ZipFile(archive, "w") as output:
                for name, content in members.items():
                    output.writestr(name, content)
            with self.assertRaisesRegex(ValueError, "build-host path") as raised:
                build_windows_setup.stage_package(archive, root / "stage")
            self.assertNotIn("builder", str(raised.exception))
            self.assertFalse((root / "stage").exists())

    def test_unknown_binary_name_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "unexpected.exe"
            binary.write_bytes(b"fixture")
            metrics = root / "metrics.json"
            metrics.write_text("{}")
            output = root / "fixture.zip"
            result = subprocess.run([sys.executable, str(PACKAGER), str(binary), str(metrics), str(output)],
                                    cwd=ROOT, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("binary must be named", result.stderr)
            self.assertFalse(output.exists())


class ReleaseBuildFlags(unittest.TestCase):
    def test_remap_rules_survive_spaces_in_order_from_general_to_specific(self):
        home = "/srv/build er"
        rules = release_build.remap_rules(home=home, cargo_home=home + "/.cargo", rustup_home=home + "/.rustup",
                                          root=home + "/src/token ashtray", target_dir="/var/cache/tok target")
        flags = release_build.rustflags(rules, "x86_64-unknown-linux-gnu")
        self.assertEqual(flags, [
            f"--remap-path-prefix={home}=~",
            f"--remap-path-prefix={home}/.cargo=/cargo",
            f"--remap-path-prefix={home}/.rustup=/rustup",
            f"--remap-path-prefix={home}/src/token ashtray=/token-ashtray",
            "--remap-path-prefix=/var/cache/tok target=/target",
        ])
        env = release_build.build_environment({"PATH": "unchanged"}, flags)
        self.assertEqual(env["PATH"], "unchanged")
        self.assertEqual(env["CARGO_ENCODED_RUSTFLAGS"].split("\x1f"), flags)
        self.assertNotIn("RUSTFLAGS", env)

    def test_windows_rules_cover_both_drive_letter_cases_and_static_crt(self):
        home = BS.join(["D:", "Build Home"])
        rules = release_build.remap_rules(home=home, cargo_home=home + BS + ".cargo",
                                          rustup_home=home + BS + ".rustup", root=BS.join(["E:", "src", "ashtray"]),
                                          target_dir=BS.join(["E:", "src", "ashtray", "target"]))
        flags = release_build.rustflags(rules, "x86_64-pc-windows-msvc")
        self.assertEqual(flags[:2], [f"--remap-path-prefix={home}=~", "--remap-path-prefix=d" + home[1:] + "=~"])
        self.assertEqual(flags[-2:], ["-C", "target-feature=+crt-static"])
        self.assertEqual(len(flags), 12)
        gnu = release_build.rustflags(rules, "x86_64-pc-windows-gnu")
        self.assertNotIn("target-feature=+crt-static", gnu)

    def test_preset_rustflags_are_refused(self):
        for name in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS",
                     "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS"):
            with self.subTest(name=name), self.assertRaisesRegex(release_build.BuildError, name):
                release_build.refuse_preset_flags({"PATH": "x", name: "-C target-feature=+crt-static"}, [])
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / "config.toml"
            config.write_text("[build]\nrustflags = ['-C', 'target-cpu=native']\n")
            with self.assertRaisesRegex(release_build.BuildError, "rustflags"):
                release_build.refuse_preset_flags({}, [config])
            config.write_text("[build]\njobs = 4\n")
            release_build.refuse_preset_flags({}, [config, Path(directory) / "missing.toml"])

    def test_refused_config_path_hides_the_account_name(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory) / "builder"
            config = home / ".cargo" / "config.toml"
            config.parent.mkdir(parents=True)
            config.write_text("[build]\nrustflags = ['-C', 'target-cpu=native']\n")
            with mock.patch.object(release_build.Path, "home", return_value=home), \
                    self.assertRaisesRegex(release_build.BuildError, "rustflags") as raised:
                release_build.refuse_preset_flags({}, [config])
            message = str(raised.exception)
            self.assertNotIn("builder", message)
            self.assertIn("~", message)
            self.assertIn("config.toml", message)

    def test_command_refuses_preset_rustflags_before_building(self):
        result = subprocess.run([sys.executable, str(ROOT / "scripts/release_build.py"), "--bin", "token-ashtray"],
                                env={**os.environ, "RUSTFLAGS": "-C target-feature=+crt-static"},
                                cwd=ROOT, capture_output=True, text=True, timeout=60)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("RUSTFLAGS", result.stderr)
        self.assertNotIn("Compiling", result.stderr)


@unittest.skipUnless(PWSH, "PowerShell is unavailable")
class PowerShellInstall(unittest.TestCase):
    def test_parser_and_disposable_file_ownership_helpers(self):
        # Extract pure helper functions from the AST. The main installer requires
        # native Windows COM and HKCU, so it is not invoked on a Linux profile.
        harness = r"""
$ErrorActionPreference = 'Stop'
$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($env:INSTALL_SCRIPT, [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw ($errors | ForEach-Object Message) }
$names = @('FullPath', 'SamePath', 'CheckRegularFile', 'Installed', 'CommandFor', 'CopyAtomic',
           'RemoveOwnedFiles', 'RemoveSettingsFile', 'ReadShortcut', 'RemoveStartupSwitch', 'RunningFrom')
$functions = $ast.FindAll({ param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
    $names -contains $node.Name
}, $true)
if ($functions.Count -ne $names.Count) { throw 'Missing helper functions' }
foreach ($function in $functions) { Invoke-Expression $function.Extent.Text }
$markerName = '.token-ashtray-install'
$markerText = "Token Ashtray per-user installation`n"
$shortcutDescription = 'Token Ashtray per-user launcher'
$directory = Join-Path $env:FIXTURE_ROOT 'installed app'
$null = [IO.Directory]::CreateDirectory($directory)
if (Installed $directory) { throw 'Unmarked directory was accepted' }
$marker = Join-Path $directory $markerName
[IO.File]::WriteAllText($marker, $markerText)
if (-not (Installed $directory)) { throw 'Owned directory was rejected' }
$source = Join-Path $env:FIXTURE_ROOT 'source fixture'
$binary = Join-Path $directory 'token-ashtray.exe'
if ((CommandFor $directory) -ne ('"' + $binary + '" --autostart')) {
    throw 'Run value does not quote a path with spaces'
}
[IO.File]::WriteAllText($source, 'v1')
CopyAtomic $source $binary
[IO.File]::WriteAllText($source, 'v2')
CopyAtomic $source $binary
if ([IO.File]::ReadAllText($binary) -ne 'v2') { throw 'Repeat replacement failed' }
$other = Join-Path $directory 'unrelated.txt'
[IO.File]::WriteAllText($other, 'preserve')
RemoveOwnedFiles $directory
if (-not [IO.File]::Exists($other) -or [IO.File]::Exists($binary) -or [IO.File]::Exists($marker)) {
    throw 'Removal touched unrelated file or kept an app-owned file'
}
RemoveOwnedFiles $directory
if (-not [IO.File]::Exists($other)) { throw 'Repeat removal touched unrelated file' }
[IO.File]::WriteAllText($marker, 'unrelated')
$markerRejected = $false
try { $null = Installed $directory }
catch {
    if ($_.Exception.Message -notlike '*Installation marker is unrelated*') { throw }
    $markerRejected = $true
}
if (-not $markerRejected) { throw 'Bad marker guard did not reject' }
$shortcutPath = Join-Path $env:FIXTURE_ROOT 'Token Ashtray.lnk'
[IO.File]::WriteAllText($shortcutPath, 'fake shortcut file')
$script:fakeShortcut = [pscustomobject]@{
    TargetPath = $binary
    Description = $shortcutDescription
    Arguments = ''
}
$script:fakeShell = [pscustomobject]@{}
$script:fakeShell | Add-Member -MemberType ScriptMethod -Name CreateShortcut -Value {
    param($path) return $script:fakeShortcut
}
function New-Object { param([string]$ComObject)
    if ($ComObject -ne 'WScript.Shell') { throw 'Unexpected COM request' }
    return $script:fakeShell
}
if (-not (ReadShortcut $shortcutPath @($binary))) { throw 'Owned shortcut was rejected' }
$script:fakeShortcut.Description = 'unrelated'
$descriptionRejected = $false
try { $null = ReadShortcut $shortcutPath @($binary) }
catch {
    if ($_.Exception.Message -notlike '*Unrelated or stale Start menu shortcut*') { throw }
    $descriptionRejected = $true
}
if (-not $descriptionRejected) { throw 'Bad shortcut description guard did not reject' }
$script:fakeShortcut.Description = $shortcutDescription
$script:fakeShortcut.Arguments = '--autostart'
$argumentsRejected = $false
try { $null = ReadShortcut $shortcutPath @($binary) }
catch {
    if ($_.Exception.Message -notlike '*Unrelated or stale Start menu shortcut*') { throw }
    $argumentsRejected = $true
}
if (-not $argumentsRejected) { throw 'Unexpected shortcut arguments were accepted' }
$settingsDir = Join-Path $env:FIXTURE_ROOT 'rammi config'
$null = [IO.Directory]::CreateDirectory($settingsDir)
$settings = Join-Path $settingsDir 'config.toml'
$neighbor = Join-Path $settingsDir 'other-file'
[IO.File]::WriteAllText($settings, 'preferences')
[IO.File]::WriteAllText($neighbor, 'preserve')
RemoveSettingsFile $settings
if ([IO.File]::Exists($settings) -or -not [IO.File]::Exists($neighbor)) {
    throw 'Preference removal touched the wrong file'
}
RemoveSettingsFile $settings
# Uninstall must clear the Windows Startup apps switch stored under the Run value's name.
$approvedAssignment = @($ast.FindAll({ param($node)
    $node -is [System.Management.Automation.Language.AssignmentStatementAst] -and
    $node.Left.Extent.Text -eq '$startupApprovedPath'
}, $true))
if ($approvedAssignment.Count -ne 1 -or (Invoke-Expression $approvedAssignment[0].Right.Extent.Text) -ne
    'Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run') {
    throw 'StartupApproved path is not the Windows Startup apps key'
}
$switchCalls = @($ast.FindAll({ param($node)
    $node -is [System.Management.Automation.Language.CommandAst] -and
    $node.GetCommandName() -eq 'RemoveStartupSwitch'
}, $true))
$uninstallBranches = @($ast.FindAll({ param($node)
    $node -is [System.Management.Automation.Language.IfStatementAst] -and
    $node.Clauses[0].Item1.Extent.Text -eq "`$Action -eq 'Install'" -and $null -ne $node.ElseClause
}, $true) | ForEach-Object { $_.ElseClause })
if ($switchCalls.Count -ne 1 -or $uninstallBranches.Count -ne 1 -or
    ($switchCalls[0].CommandElements | Select-Object -Skip 1 | ForEach-Object { $_.Extent.Text }) -join ' ' -ne
        '$startupApprovedPath $runName' -or
    $switchCalls[0].Extent.StartOffset -lt $uninstallBranches[0].Extent.StartOffset -or
    $switchCalls[0].Extent.EndOffset -gt $uninstallBranches[0].Extent.EndOffset) {
    throw 'Uninstall does not remove the app Startup apps switch'
}
if ($IsWindows) {
    $testPath = 'Software\token-ashtray-test-' + [guid]::NewGuid().ToString('N')
    $approvedPath = $testPath + '\StartupApproved\Run'
    try {
        RemoveStartupSwitch $approvedPath 'Token Ashtray'
        $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($approvedPath)
        $disabled = [byte[]](3, 0, 0, 0, 0x58, 0x2F, 0xA4, 0xC7, 0x8F, 0x76, 0xDD, 1)
        $key.SetValue('Token Ashtray', $disabled, [Microsoft.Win32.RegistryValueKind]::Binary)
        $key.SetValue('Other', $disabled, [Microsoft.Win32.RegistryValueKind]::Binary)
        $key.Close()
        RemoveStartupSwitch $approvedPath 'Token Ashtray'
        RemoveStartupSwitch $approvedPath 'Token Ashtray'
        $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($approvedPath)
        $valueNames = $key.GetValueNames()
        $other = $key.GetValue('Other')
        $key.Close()
        if ($valueNames -contains 'Token Ashtray') { throw 'Startup apps switch was kept' }
        if ($valueNames -notcontains 'Other' -or
            [BitConverter]::ToString($other) -ne [BitConverter]::ToString($disabled)) {
            throw 'Startup apps switch removal touched another value'
        }
    } finally {
        [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($testPath, $false)
    }
}
# Setup must refuse to touch an installation whose executable is running.
$elsewhere = Join-Path $env:FIXTURE_ROOT 'elsewhere'
$script:fakeProcesses = @(
    [pscustomobject]@{ Id = 11; Path = (Join-Path $directory 'token-ashtray.exe') },
    [pscustomobject]@{ Id = 12; Path = $null },
    [pscustomobject]@{ Id = 13; Path = (Join-Path $elsewhere 'token-ashtray.exe') },
    [pscustomobject]@{ Id = 14; Path = (Join-Path $directory.ToUpperInvariant() 'token-ashtray.exe') },
    [pscustomobject]@{ Id = 15; Path = (Join-Path (Join-Path $directory 'nested') 'token-ashtray.exe') }
)
function Get-Process { [CmdletBinding()] param([string]$Name)
    if ($Name -ne 'token-ashtray') { throw 'Unexpected process query' }
    return $script:fakeProcesses
}
if ((@(RunningFrom @($elsewhere, $directory)) | Sort-Object) -join ',' -ne '11,13,14') {
    throw 'Running installed executables were not detected'
}
if ((@(RunningFrom @((Join-Path $env:FIXTURE_ROOT 'idle'))) -join ',') -ne '') {
    throw 'A process outside the installation blocked setup'
}
$script:fakeProcesses = @()
if (@(RunningFrom @($directory)).Count -ne 0) { throw 'No running process was reported as running' }
function OffsetOf([System.Management.Automation.Language.Ast]$Node) { return $Node.Extent.StartOffset }
function InFunction([System.Management.Automation.Language.Ast]$Node) {
    for ($parent = $Node.Parent; $parent; $parent = $parent.Parent) {
        if ($parent -is [System.Management.Automation.Language.FunctionDefinitionAst]) { return $true }
    }
    return $false
}
$runningCalls = @($ast.FindAll({ param($node)
    $node -is [System.Management.Automation.Language.CommandAst] -and
    $node.GetCommandName() -eq 'RunningFrom'
}, $true))
$runningGuards = @($ast.FindAll({ param($node)
    $node -is [System.Management.Automation.Language.IfStatementAst] -and
    $node.Clauses[0].Item1.Extent.Text -eq '$running.Count -gt 0' -and
    @($node.Clauses[0].Item2.FindAll({ param($inner)
        $inner -is [System.Management.Automation.Language.ThrowStatementAst] }, $true)).Count -eq 1
}, $true))
# The earliest file, shortcut or registry change in the script body (not inside a helper).
$firstMutation = @($ast.FindAll({ param($node)
    ($node -is [System.Management.Automation.Language.InvokeMemberExpressionAst] -and
     @('CreateDirectory', 'WriteAllText', 'Delete', 'DeleteValue', 'SetValue', 'Save') -contains $node.Member.Value) -or
    ($node -is [System.Management.Automation.Language.CommandAst] -and
     @('CopyAtomic', 'RemoveOwnedFiles', 'RemoveStartupSwitch', 'RemoveSettingsFile') -contains $node.GetCommandName())
}, $true) | Where-Object { -not (InFunction $_) } | Sort-Object { OffsetOf $_ } | Select-Object -First 1)
if ($runningCalls.Count -ne 1 -or $runningGuards.Count -ne 1 -or
    $runningCalls[0].Extent.Text -ne 'RunningFrom @($destination, $previous)' -or
    $firstMutation.Count -ne 1 -or
    (OffsetOf $runningGuards[0]) -lt (OffsetOf $runningCalls[0]) -or
    $runningGuards[0].Extent.EndOffset -gt (OffsetOf $firstMutation[0])) {
    throw 'Setup does not stop for a running app before its first change'
}
$keptMessages = @($ast.FindAll({ param($node)
    $node -is [System.Management.Automation.Language.CommandAst] -and
    $node.GetCommandName() -eq 'Write-Output' -and $node.Extent.Text -like '*Kept app preferences*'
}, $true))
$keptClause = @(if ($keptMessages.Count -eq 1) {
    $parent = $keptMessages[0].Parent
    while ($parent -and $parent -isnot [System.Management.Automation.Language.IfStatementAst]) { $parent = $parent.Parent }
    $parent.Clauses | Where-Object {
        $_.Item2.Extent.StartOffset -le $keptMessages[0].Extent.StartOffset -and
        $_.Item2.Extent.EndOffset -ge $keptMessages[0].Extent.EndOffset }
})
if ($keptClause.Count -ne 1 -or $keptClause[0].Item1.Extent.Text -ne '[IO.File]::Exists($settings)') {
    throw 'Kept preferences message is printed without a settings file'
}
Write-Output 'isolated helpers OK'
"""
        with tempfile.TemporaryDirectory(prefix="windows helper path ") as directory:
            root = Path(directory)
            script = root / "fixture.ps1"
            script.write_text(harness)
            env = {**os.environ, "INSTALL_SCRIPT": os.environ.get(
                       "TOKEN_ASHTRAY_TEST_INSTALL_SCRIPT", str(ROOT / "install-windows.ps1")),
                   "FIXTURE_ROOT": str(root), "HOME": str(root / "home"),
                   "XDG_CONFIG_HOME": str(root / "config"),
                   "POWERSHELL_TELEMETRY_OPTOUT": "1", "DOTNET_CLI_TELEMETRY_OPTOUT": "1"}
            result = subprocess.run([PWSH, "-NoLogo", "-NoProfile", "-NonInteractive", "-File", str(script)],
                                    env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("isolated helpers OK", result.stdout)


@unittest.skipUnless(os.name == "posix", "Linux XDG fixtures require POSIX")
class LinuxInstall(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="install path ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.archive = self.root / "archive"
        self.archive.mkdir()
        shutil.copy2(LINUX_HELPER, self.archive / LINUX_HELPER.name)
        (self.archive / "token-ashtray").write_text("fixture v1")
        (self.archive / "token-ashtray-256.png").write_bytes(b"image")
        self.home = self.root / "home space"
        self.home.mkdir()
        self.data = self.home / "data space"
        self.config = self.home / "config space"
        self.env = {**os.environ, "HOME": str(self.home), "XDG_DATA_HOME": str(self.data),
                    "XDG_CONFIG_HOME": str(self.config)}
        self.destination = self.home / "installed app"
        self.launcher = self.data / "applications/token-ashtray.desktop"
        self.autostart = self.config / "autostart/token-ashtray.desktop"

    def run_helper(self, *args, success=True):
        result = subprocess.run([sys.executable, str(self.archive / LINUX_HELPER.name), *args], env=self.env,
                                cwd=self.archive, capture_output=True, text=True)
        if success:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0)
        return result

    def test_install_update_relocate_and_remove(self):
        old = self.destination
        self.run_helper("install", "--install-dir", str(old))
        self.assertTrue((old / "token-ashtray").exists())
        self.assertTrue(os.access(old / "token-ashtray", os.X_OK))
        self.assertIn(str(old / "token-ashtray"), self.launcher.read_text())
        if shutil.which("desktop-file-validate"):
            validation = subprocess.run(["desktop-file-validate", str(self.launcher)],
                                        capture_output=True, text=True)
            self.assertEqual(validation.returncode, 0, validation.stderr)
        self.assertFalse(self.autostart.exists())
        settings = self.config / "token-ashtray/config.toml"
        settings.parent.mkdir(parents=True)
        preferences = ('unknown = "keep"\n[board]\nquota_display_mode = "used"\n'
                       'provider_order = ["codex", "claude"]\n'
                       '[panels.example]\ncompact = true\n')
        settings.write_text(preferences)
        credential = self.home / ".claude" / "credentials.json"
        credential.parent.mkdir()
        credential.write_text("do not touch")
        self.autostart.parent.mkdir(parents=True)
        other = self.autostart.parent / "other.desktop"
        other.write_text("other")
        self.autostart.write_text("[Desktop Entry]\nType=Application\nX-Token-Ashtray=true\n"
                                  f'Exec="{old / "token-ashtray"}" --autostart\n')
        (self.archive / "token-ashtray").write_text("fixture v2")
        self.run_helper("install", "--install-dir", str(old))
        self.assertEqual((old / "token-ashtray").read_text(), "fixture v2")
        new = self.home / "new location space"
        self.run_helper("install", "--install-dir", str(new), "--previous-dir", str(old))
        self.assertIn(str(new / "token-ashtray"), self.autostart.read_text())
        self.assertNotIn(str(old / "token-ashtray"), self.autostart.read_text())
        self.assertFalse(old.exists())
        self.assertEqual(settings.read_text(), preferences)
        self.run_helper("uninstall", "--install-dir", str(new))
        self.assertFalse(new.exists())
        self.assertFalse(self.launcher.exists())
        self.assertFalse(self.autostart.exists())
        self.assertEqual(other.read_text(), "other")
        self.assertTrue(settings.exists())
        self.assertEqual(credential.read_text(), "do not touch")
        self.run_helper("uninstall", "--install-dir", str(new), "--remove-settings")
        self.assertFalse(settings.exists())
        self.assertEqual(credential.read_text(), "do not touch")

    def test_unrelated_named_entries_are_preserved(self):
        self.launcher.parent.mkdir(parents=True)
        self.launcher.write_text("[Desktop Entry]\nName=Other\n")
        result = self.run_helper("install", "--install-dir", str(self.destination), success=False)
        self.assertIn("unrelated", result.stderr.lower())
        self.assertEqual(self.launcher.read_text(), "[Desktop Entry]\nName=Other\n")
        self.launcher.unlink()
        self.run_helper("install", "--install-dir", str(self.destination))
        self.autostart.parent.mkdir(parents=True)
        self.autostart.write_text("[Desktop Entry]\nName=Other\n")
        result = self.run_helper("uninstall", "--install-dir", str(self.destination), success=False)
        self.assertIn("unrelated", result.stderr.lower())
        self.assertTrue(self.destination.exists())

    def test_disabled_autostart_stays_disabled_after_move(self):
        old = self.destination
        self.run_helper("install", "--install-dir", str(old))
        self.autostart.parent.mkdir(parents=True)
        self.autostart.write_text("[Desktop Entry]\nType=Application\nX-Token-Ashtray=true\n"
                                  "Hidden=true\n"
                                  f'Exec="{old / "token-ashtray"}" --autostart\n')
        new = self.home / "relocated app"
        self.run_helper("install", "--install-dir", str(new), "--previous-dir", str(old))
        body = self.autostart.read_text()
        self.assertIn("Hidden=true", body)
        self.assertIn(str(new / "token-ashtray"), body)
        self.run_helper("install", "--install-dir", str(new))
        self.assertEqual(self.autostart.read_text(), body)

    def test_only_app_created_empty_autostart_directory_is_removed(self):
        self.run_helper("install", "--install-dir", str(self.destination))
        self.autostart.parent.mkdir(parents=True)
        info = self.autostart.parent.stat()
        self.autostart.write_text("[Desktop Entry]\nType=Application\nX-Token-Ashtray=true\n"
                                  f'Exec="{self.destination / "token-ashtray"}" --autostart\n'
                                  f"X-Token-Ashtray-Created-Autostart-Dir={info.st_dev}:{info.st_ino}\n")
        self.run_helper("uninstall", "--install-dir", str(self.destination))
        self.assertFalse(self.autostart.parent.exists())

    def test_repeat_removal_keeps_unrelated_install_folder_files(self):
        self.run_helper("install", "--install-dir", str(self.destination))
        unrelated = self.destination / "unrelated.txt"
        unrelated.write_text("keep")
        self.run_helper("uninstall", "--install-dir", str(self.destination))
        self.assertEqual(unrelated.read_text(), "keep")
        self.assertFalse((self.destination / "token-ashtray").exists())
        self.run_helper("uninstall", "--install-dir", str(self.destination))
        self.assertEqual(unrelated.read_text(), "keep")

    @unittest.skipIf(hasattr(os, "geteuid") and os.geteuid() == 0,
                     "root bypasses directory write permissions")
    def test_permission_error_is_reported_without_claiming_removal(self):
        self.run_helper("install", "--install-dir", str(self.destination))
        self.autostart.parent.mkdir(parents=True)
        self.autostart.write_text("[Desktop Entry]\nType=Application\nX-Token-Ashtray=true\n"
                                  f'Exec="{self.destination / "token-ashtray"}" --autostart\n')
        self.autostart.parent.chmod(0o500)
        try:
            result = self.run_helper("uninstall", "--install-dir", str(self.destination), success=False)
            self.assertIn("permission", result.stderr.lower())
            self.assertTrue(self.autostart.exists())
            self.assertTrue(self.destination.exists())
        finally:
            self.autostart.parent.chmod(0o700)


if __name__ == "__main__":
    unittest.main()
