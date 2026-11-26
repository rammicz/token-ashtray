"""Synthetic headers and package bytes only: never compile, execute, or fetch."""
import io
import json
from pathlib import Path
import stat
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
import zipfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import artifact_descriptor as descriptor
import package_linux_setup as linux_setup

SOURCE = "a" * 40
LINUX = "x86_64-unknown-linux-gnu"
WINDOWS = "x86_64-pc-windows-msvc"


def elf(machine=62):
    header = struct.pack("<16sHHIQQQIHHHHHH", b"\x7fELF\x02\x01\x01" + b"\0" * 9,
                         3, machine, 1, 0x400080, 64, 0, 0, 64, 56, 1, 0, 0, 0)
    program = struct.pack("<IIQQQQQQ", 1, 5, 0, 0x400000, 0x400000, 128, 128, 4096)
    return header + program + b"SYNTHETIC"[:8]


def pe(machine=0x8664):
    data = bytearray(528)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 60, 64)
    data[64:68] = b"PE\0\0"
    struct.pack_into("<HHIIIHH", data, 68, machine, 1, 0, 0, 0, 240, 2)
    struct.pack_into("<H", data, 88, 0x20b)
    struct.pack_into("<II", data, 88 + 56, 8192, 512)
    struct.pack_into("<H", data, 88 + 68, 2)
    data[328:336] = b".text\0\0\0"
    struct.pack_into("<IIII", data, 336, 16, 4096, 16, 512)
    data[512:] = b"SYNTHETIC ONLY!!"
    return bytes(data)


def setup_pe(machine=0x14c, magic=0x10b, subsystem=2, flags=0x102):
    """A synthetic Inno Setup-like stub: 32-bit x86 GUI PE."""
    data = bytearray(512)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 60, 64)
    data[64:68] = b"PE\0\0"
    struct.pack_into("<HHIIIHH", data, 68, machine, 1, 0, 0, 0, 224, flags)
    struct.pack_into("<H", data, 88, magic)
    struct.pack_into("<H", data, 88 + 68, subsystem)
    data[312:320] = b".text\0\0\0"
    data[400:] = b"SYNTHETIC SETUP ONLY".ljust(112, b"!")
    return bytes(data)


class ArtifactDescriptor(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.references = self.root / "references"
        self.references.mkdir()
        for name in ("LICENSE", "THIRD-PARTY.md", "INSTALL-LINUX.md", "INSTALL-WINDOWS.md", "install-windows.ps1"):
            (self.references / name).write_text("reference " + name)
        (self.references / "docs").mkdir()
        (self.references / "docs/LICENSE-REVIEW.md").write_text("reference review")
        (self.references / "assets/application").mkdir(parents=True)
        (self.references / "assets/application/token-ashtray-256.png").write_bytes(b"reference icon")
        self.limits = descriptor.Limits(100000, 10000, 50000, 10000)

    def members(self, target):
        app = elf() if target == LINUX else pe()
        resources = {name: (self.references / path).read_bytes()
                     for name, path in descriptor.REFERENCES[target].items()}
        resources[descriptor.APPS[target]] = app
        resources["RELEASE-METRICS.json"] = descriptor.canonical({
            "binary_sha256": descriptor.sha(app), "binary_bytes": len(app)})
        return resources

    def linux(self, members=None, edit=None, stub=None):
        members = self.members(LINUX) if members is None else members
        stub = elf() if stub is None else stub
        manifest = {"format": 1, "source_sha": SOURCE, "stub_size": len(stub),
                    "stub_sha256": descriptor.sha(stub), "members": [
                        {"name": name, "size": len(body), "sha256": descriptor.sha(body)}
                        for name, body in members.items()]}
        if edit:
            edit(manifest)
        raw = descriptor.canonical(manifest)
        data = stub + b"".join(members.values()) + raw + struct.pack("<Q", len(raw)) + descriptor.MAGIC
        return data

    def windows(self, members=None, special=None, compression=zipfile.ZIP_DEFLATED):
        members = self.members(WINDOWS) if members is None else members
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            for name, body in members.items():
                entry = zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0))
                entry.create_system = 3
                entry.external_attr = (stat.S_IFREG | 0o644) << 16
                entry.compress_type = compression
                if special:
                    special(entry)
                archive.writestr(entry, body)
        return buffer.getvalue()

    def inspect(self, data, target=LINUX, setup=None, **kwargs):
        """Inspects Linux `data`, or a Windows setup built from the ZIP `data`."""
        artifact = self.root / "input"
        options = dict(target=target, version="0.2.2", source_sha=SOURCE,
                       resources=self.references, limits=self.limits)
        if target == WINDOWS:
            artifact.write_bytes(setup_pe() if setup is None else setup)
            options["payload"] = self.root / "payload.zip"
            options["payload"].write_bytes(data)
        else:
            artifact.write_bytes(data)
        options.update(kwargs)
        return descriptor.inspect_artifact(artifact, **options)

    def test_both_formats_and_deterministic_unsigned_output(self):
        for target, data in ((LINUX, self.linux()), (WINDOWS, self.windows())):
            with self.subTest(target=target):
                first = self.inspect(data, target)
                self.assertEqual(descriptor.canonical(first), descriptor.canonical(self.inspect(data, target)))
                self.assertEqual(first["status"], "unsigned-candidate")
                self.assertEqual(first["purpose"], "installer")
                artifact = data if target == LINUX else setup_pe()
                self.assertEqual(first["artifact"]["sha256"], descriptor.sha(artifact))
                self.assertEqual(len(first["members"]), 7)
                self.assertEqual(first["assertions"]["compile_provenance"], "not_verified")
                self.assertEqual(first["assertions"]["version"], "caller_asserted")
                self.assertEqual(first["app"]["architecture"], "x86_64")
                self.assertNotIn("signatures", first)
                if target == LINUX:
                    self.assertEqual(first["linux_setup"]["stub"]["sha256"], descriptor.sha(elf()))
                    self.assertEqual(first["linux_setup"]["source_sha_assertion"], SOURCE)
                    trailer = first["linux_setup"]["raw_trailer"]
                    self.assertEqual(trailer["sha256"], descriptor.sha(data[trailer["offset"]:]))
                    self.assertEqual(trailer["length"], len(data) - trailer["offset"])
                else:
                    self.assertEqual(first["artifact_kind"], "windows-setup-exe-v1")
                    setup = first["windows_setup"]
                    self.assertEqual(setup["payload_zip"]["sha256"], descriptor.sha(data))
                    self.assertEqual(setup["payload_relation"], "caller_asserted_build_input")
                    self.assertEqual((setup["stub"]["format"], setup["stub"]["architecture"]), ("PE32", "x86"))

    def test_windows_setup_headers_and_payload_are_required(self):
        for setup in (setup_pe(machine=0x8664), setup_pe(magic=0x20b), setup_pe(subsystem=3),
                      setup_pe(flags=0x2102), pe(), setup_pe()[:300], b"not a program"):
            with self.subTest(setup=setup[:8]), self.assertRaises(descriptor.Reject):
                self.inspect(self.windows(), WINDOWS, setup=setup)
        with self.assertRaisesRegex(descriptor.Reject, "portable ZIP"):
            self.inspect(self.windows(), WINDOWS, payload=None)

    def test_caller_assertions_and_bounds(self):
        for options in ({"version": "01.2.3"}, {"version": "0.2.2-beta"},
                        {"source_sha": "A" * 40}, {"purpose": "update"},
                        {"target": "aarch64-unknown-linux-gnu"},
                        {"limits": descriptor.Limits(10, 10000, 50000, 10000)},
                        {"limits": descriptor.Limits(100000, 100, 50000, 10000)},
                        {"limits": descriptor.Limits(100000, 10000, 100, 10000)},
                        {"limits": descriptor.Limits(100000, 10000, 50000, 10)},
                        {"limits": descriptor.Limits(0, 10000, 50000, 10000)}):
            with self.subTest(options=options), self.assertRaises(descriptor.Reject):
                self.inspect(self.linux(), **options)

    def test_linux_integrity_source_lengths_and_resources(self):
        changes = [lambda m: m.update(source_sha="b" * 40),
                   lambda m: m.update(stub_sha256="0" * 64),
                   lambda m: m.update(stub_size=True),
                   lambda m: m["members"][0].update(size=1000000),
                   lambda m: m["members"][0].update(sha256="0" * 64),
                   lambda m: m["members"][1].update(name=m["members"][0]["name"]),
                   lambda m: m["members"].pop(),
                   lambda m: m.update(format=2)]
        for edit in changes:
            with self.subTest(edit=edit), self.assertRaises(descriptor.Reject):
                self.inspect(self.linux(edit=edit))
        data = self.linux()
        for broken in (data[:-1], data + b"extra", data[:-len(descriptor.MAGIC)-8] + struct.pack("<Q", 2**63) + descriptor.MAGIC):
            with self.subTest(truncated=True), self.assertRaises(descriptor.Reject):
                self.inspect(broken)
        # Hash-bound, internally consistent resource changes still must match reviewed bytes.
        for name in ("LICENSE", "RELEASE-METRICS.json", "token-ashtray"):
            members = self.members(LINUX)
            members[name] += b"changed"
            with self.subTest(member=name), self.assertRaises(descriptor.Reject):
                self.inspect(self.linux(members))

    def test_linux_exact_set_consumption_and_json(self):
        for change in (lambda m: m.pop("LICENSE"),
                       lambda m: m.update(extra=b"unexpected"),
                       lambda m: m.update(LICENSE=b"")):
            members = self.members(LINUX)
            change(members)
            with self.subTest(change=change), self.assertRaises(descriptor.Reject):
                self.inspect(self.linux(members))
        data = self.linux()
        end = len(data) - len(descriptor.MAGIC) - 8
        start = end - struct.unpack_from("<Q", data, end)[0]
        with self.assertRaises(descriptor.Reject):
            self.inspect(data[:start] + b"gap" + data[start:])
        for raw in (b'{"format":1,"format":1}', b'[]', b'{"format":NaN}'):
            malformed = data[:start] + raw + struct.pack("<Q", len(raw)) + descriptor.MAGIC
            with self.subTest(json=raw), self.assertRaises(descriptor.Reject):
                self.inspect(malformed)

    def test_provenance_is_not_inferred_from_binary_headers(self):
        result = self.inspect(self.windows(), WINDOWS, version="9.8.7", source_sha="b" * 40)
        self.assertEqual(result["version"], "9.8.7")
        self.assertEqual(result["source_sha"], "b" * 40)
        self.assertEqual(result["assertions"]["target_toolchain"], "caller_asserted")
        self.assertEqual(result["assertions"]["reference_resource_source_relation"], "caller_asserted")

    def test_wrong_executable_architecture_and_kind(self):
        with self.assertRaises(descriptor.Reject):
            self.inspect(self.linux(stub=elf(3)))
        for target, bad_app in ((LINUX, elf(3)), (WINDOWS, pe(0x14c)), (WINDOWS, elf())):
            members = self.members(target)
            members[descriptor.APPS[target]] = bad_app
            members["RELEASE-METRICS.json"] = descriptor.canonical({"binary_sha256": descriptor.sha(bad_app), "binary_bytes": len(bad_app)})
            with self.subTest(target=target), self.assertRaises(descriptor.Reject):
                self.inspect(self.linux(members) if target == LINUX else self.windows(members), target)
        with self.assertRaises(descriptor.Reject):
            self.inspect(self.windows(), LINUX)
        with self.assertRaises(descriptor.Reject):
            self.inspect(self.linux(), WINDOWS)

    def test_metrics_identity_and_malformed_executable_headers(self):
        for target in (LINUX, WINDOWS):
            for values in ({"binary_sha256": "0" * 64, "binary_bytes": 1},
                           {"binary_sha256": descriptor.sha(self.members(target)[descriptor.APPS[target]]), "binary_bytes": True}):
                members = self.members(target)
                members["RELEASE-METRICS.json"] = descriptor.canonical(values)
                with self.subTest(target=target, metrics=values), self.assertRaises(descriptor.Reject):
                    self.inspect(self.linux(members) if target == LINUX else self.windows(members), target)
            members = self.members(target)
            app = members[descriptor.APPS[target]][:64]
            members[descriptor.APPS[target]] = app
            members["RELEASE-METRICS.json"] = descriptor.canonical({"binary_sha256": descriptor.sha(app), "binary_bytes": len(app)})
            with self.subTest(target=target, truncated=True), self.assertRaises(descriptor.Reject):
                self.inspect(self.linux(members) if target == LINUX else self.windows(members), target)

    def test_zip_layout_types_modes_compression_and_metrics(self):
        for replacement in ("../LICENSE", "/LICENSE", "extra", "folder/LICENSE"):
            members = self.members(WINDOWS)
            members[replacement] = members.pop("LICENSE")
            with self.subTest(name=replacement), self.assertRaises(descriptor.Reject):
                self.inspect(self.windows(members), WINDOWS)
        for mode in (stat.S_IFLNK | 0o644, stat.S_IFDIR | 0o644, stat.S_IFREG | 0o755):
            def change(entry):
                entry.external_attr = mode << 16
            with self.subTest(mode=mode), self.assertRaises(descriptor.Reject):
                self.inspect(self.windows(special=change), WINDOWS)
        with self.assertRaises(descriptor.Reject):
            self.inspect(self.windows(compression=zipfile.ZIP_BZIP2), WINDOWS)
        for name in ("LICENSE", "RELEASE-METRICS.json"):
            members = self.members(WINDOWS)
            members[name] = b"{}"
            with self.subTest(member=name), self.assertRaises(descriptor.Reject):
                self.inspect(self.windows(members), WINDOWS)
        data = self.windows()
        for broken in (data[:-1], data + b"extra", data.replace(b"LICENSE", b"THIRDXX", 1)):
            with self.subTest(broken=True), self.assertRaises(descriptor.Reject):
                self.inspect(broken, WINDOWS)

    def test_zip_missing_duplicate_empty_and_historical_layout(self):
        members = self.members(WINDOWS)
        members.pop("LICENSE")
        with self.assertRaises(descriptor.Reject):
            self.inspect(self.windows(members), WINDOWS)
        members = self.members(WINDOWS)
        members["LICENSE"] = b""
        with self.assertRaises(descriptor.Reject):
            self.inspect(self.windows(members), WINDOWS)
        def duplicate(entry):
            if entry.filename == "THIRD-PARTY.md":
                entry.filename = "LICENSE"
        with self.assertWarns(UserWarning):
            data = self.windows(special=duplicate)
        with self.assertRaises(descriptor.Reject):
            self.inspect(data, WINDOWS)
        historical = {"old-layout/token-ashtray.exe": pe(), "old-layout/THIRD-PARTY.md": b"fixture", "old-layout/release-manifest.json": b"{}"}
        with self.assertRaises(descriptor.Reject):
            self.inspect(self.windows(historical), WINDOWS)

    def test_zip_actual_decompression_bounded_and_no_hidden_output(self):
        members = self.members(WINDOWS)
        members["LICENSE"] = b"x" * 20000
        with self.assertRaises(descriptor.Reject):
            self.inspect(self.windows(members), WINDOWS)
        # Forge a smaller declared size in both records. Actual DEFLATE output must be checked.
        data = bytearray(self.windows(members))
        name = b"LICENSE"
        offset = 0
        while data[offset:offset+4] == b"PK\x03\x04":
            fields = struct.unpack_from("<4s5H3I2H", data, offset)
            start = offset + 30
            if data[start:start+fields[9]] == name:
                struct.pack_into("<I", data, offset + 22, 1)
                break
            offset = start + fields[9] + fields[10] + fields[7]
        central = data.index(b"PK\x01\x02")
        while data[central:central+4] == b"PK\x01\x02":
            fields = struct.unpack_from("<4s6H3I5H2I", data, central)
            if data[central+46:central+46+fields[10]] == name:
                struct.pack_into("<I", data, central + 24, 1)
                break
            central += 46 + fields[10] + fields[11] + fields[12]
        with self.assertRaises(descriptor.Reject):
            self.inspect(bytes(data), WINDOWS)

    def test_zip_crc_and_caller_limits(self):
        data = self.windows(compression=zipfile.ZIP_STORED)
        changed = data.replace(b"reference LICENSE", b"Reference LICENSE", 1)
        with self.assertRaises(descriptor.Reject):
            self.inspect(changed, WINDOWS)
        for limits in (descriptor.Limits(10, 10000, 50000, 10000),
                       descriptor.Limits(100000, 100, 50000, 10000),
                       descriptor.Limits(100000, 10000, 100, 10000),
                       descriptor.Limits(100000, 10000, 50000, 10)):
            with self.subTest(limits=limits), self.assertRaises(descriptor.Reject):
                self.inspect(data, WINDOWS, limits=limits)

    def test_cli_and_existing_packagers(self):
        # Exercise current packager output with synthetic headers and full repository resource bytes.
        app, stub, metrics = self.root / "token-ashtray", self.root / "stub", self.root / "metrics.json"
        app.write_bytes(elf())
        stub.write_bytes(elf())
        metrics.write_bytes(descriptor.canonical({"binary_sha256": descriptor.sha(app.read_bytes()), "binary_bytes": app.stat().st_size}))
        out = self.root / "setup"
        subprocess.run([sys.executable, "scripts/package_linux_setup.py", str(stub), str(app), str(metrics), str(out), "--source-sha", SOURCE], cwd=ROOT, check=True, capture_output=True)
        command = [sys.executable, "scripts/artifact_descriptor.py", str(out), "--target", LINUX, "--version", "0.2.2", "--source-sha", SOURCE, "--reference-root", str(ROOT), "--max-artifact-bytes", "1000000", "--max-member-bytes", "500000", "--max-expanded-bytes", "1000000", "--max-metadata-bytes", "10000"]
        first = subprocess.run(command, cwd=ROOT, check=True, capture_output=True).stdout
        self.assertEqual(first, subprocess.run(command, cwd=ROOT, check=True, capture_output=True).stdout)
        self.assertEqual(json.loads(first)["status"], "unsigned-candidate")
        invalid = command.copy()
        invalid[invalid.index("0.2.2")] = "0.2.2-beta"
        failure = subprocess.run(invalid, cwd=ROOT, capture_output=True)
        self.assertEqual(failure.returncode, 2)
        self.assertEqual(failure.stdout, b"")
        self.assertNotIn(b"Traceback", failure.stderr)
        app = self.root / "token-ashtray.exe"
        app.write_bytes(pe())
        metrics.write_bytes(descriptor.canonical({"binary_sha256": descriptor.sha(app.read_bytes()), "binary_bytes": app.stat().st_size}))
        out = self.root / "windows.zip"
        subprocess.run([sys.executable, "scripts/package_release.py", str(app), str(metrics), str(out)], cwd=ROOT, check=True, capture_output=True)
        setup = self.root / "token-ashtray-setup-0.2.2-x86_64-pc-windows-msvc.exe"
        setup.write_bytes(setup_pe())
        command = [sys.executable, "scripts/artifact_descriptor.py", str(setup), "--target", WINDOWS, "--version", "0.2.2", "--source-sha", SOURCE, "--reference-root", str(ROOT), "--payload-zip", str(out), "--max-artifact-bytes", "1000000", "--max-member-bytes", "500000", "--max-expanded-bytes", "1000000", "--max-metadata-bytes", "10000"]
        result = json.loads(subprocess.run(command, cwd=ROOT, check=True, capture_output=True).stdout)
        self.assertEqual(result["artifact_kind"], "windows-setup-exe-v1")
        self.assertEqual(result["windows_setup"]["payload_zip"]["sha256"], descriptor.sha(out.read_bytes()))

    def test_linux_replace_failure_cleans_temp_and_preserves_output(self):
        app, stub, metrics = self.root / "token-ashtray", self.root / "stub", self.root / "metrics.json"
        app.write_bytes(elf())
        stub.write_bytes(elf())
        metrics.write_bytes(descriptor.canonical({"binary_sha256": descriptor.sha(app.read_bytes()),
                                                  "binary_bytes": app.stat().st_size}))
        out = self.root / "setup"
        out.write_bytes(b"previous release")
        arguments = ["package_linux_setup.py", str(stub), str(app), str(metrics), str(out),
                     "--source-sha", SOURCE]
        with mock.patch.object(sys, "argv", arguments), \
                mock.patch.object(linux_setup.os, "replace", side_effect=OSError("replace failed")):
            with self.assertRaisesRegex(OSError, "replace failed"):
                linux_setup.main()
        self.assertEqual(out.read_bytes(), b"previous release")
        self.assertEqual(list(self.root.glob(".setup-*")), [])


if __name__ == "__main__":
    unittest.main()
