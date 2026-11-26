"""Build a per-user graphical installer from the exact Windows release ZIP.

Use the pinned Inno Setup 6.5.4 compiler. This never downloads tools, builds
the application, signs executables, installs anything, or changes Windows policy.
"""

import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import zipfile

try:
    import tomllib
except ModuleNotFoundError:  # Python 3.10 fallback
    tomllib = None


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from path_leak import require_clean  # noqa: E402
PACKAGE_MEMBERS = {
    "token-ashtray.exe", "LICENSE", "THIRD-PARTY.md", "LICENSE-REVIEW.md",
    "RELEASE-METRICS.json", "INSTALL-WINDOWS.md", "install-windows.ps1",
}


def stage_package(archive_path, destination):
    """Validate every input before writing only the approved payload members."""
    with zipfile.ZipFile(archive_path) as archive:
        if len(archive.namelist()) != len(PACKAGE_MEMBERS) or set(archive.namelist()) != PACKAGE_MEMBERS:
            raise ValueError("Windows package members differ from the fixed release allowlist")
        payload = {name: archive.read(name) for name in PACKAGE_MEMBERS}
    if any(not content for content in payload.values()):
        raise ValueError("Windows package members must all be nonempty")
    if payload["install-windows.ps1"] != (ROOT / "install-windows.ps1").read_bytes():
        raise ValueError("Package helper must match the current read-only preflight contract")
    binary = payload["token-ashtray.exe"]
    try:
        pe = struct.unpack_from("<I", binary, 0x3C)[0]
        valid_pe = (binary[:2] == b"MZ" and binary[pe:pe + 4] == b"PE\0\0"
                    and struct.unpack_from("<H", binary, pe + 4)[0] == 0x8664
                    and struct.unpack_from("<H", binary, pe + 24)[0] == 0x20B
                    and struct.unpack_from("<H", binary, pe + 24 + 68)[0] == 2)
    except struct.error:
        valid_pe = False
    if not valid_pe:
        raise ValueError("Windows payload must be an x64 GUI PE executable")
    metrics = json.loads(payload["RELEASE-METRICS.json"])
    if hashlib.sha256(binary).hexdigest() != metrics.get("binary_sha256"):
        raise ValueError("Binary SHA-256 differs from RELEASE-METRICS.json")
    # Inno compresses the payload, so a scan of the finished setup proves nothing.
    require_clean({"token-ashtray.exe": binary})
    # A PE header alone cannot establish the compiler or CRT linkage. The
    # maintainer records the MSVC build command and checks imports separately.
    destination.mkdir(parents=True)
    for name, content in payload.items():
        (destination / name).write_bytes(content)
    (destination / ".token-ashtray-install").write_bytes(b"Token Ashtray per-user installation\n")
    (destination / ".token-ashtray-setup").write_bytes(b"Token Ashtray graphical setup resources\n")
    shutil.copyfile(ROOT / "packaging/windows/INNO-LICENSE.txt", destination / "INNO-LICENSE.txt")


def package_version(manifest_path=ROOT / "Cargo.toml"):
    """Read [package].version from Cargo.toml on Python 3.10+."""
    text = manifest_path.read_text(encoding="utf-8")
    if tomllib is not None:
        version = tomllib.loads(text).get("package", {}).get("version")
        if isinstance(version, str) and version:
            return version
        raise ValueError("Cargo.toml must define a nonempty [package].version string")
    in_package = False
    for raw_line in text.splitlines():
        line = raw_line.split("#", 1)[0].strip()
        if not line:
            continue
        if line.startswith("["):
            in_package = line == "[package]"
            continue
        if in_package:
            match = re.match(r'^version\s*=\s*(["\'])([^"\']+)\1$', line)
            if match:
                return match.group(2)
    raise ValueError("Cargo.toml must define a nonempty [package].version string")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path)
    parser.add_argument("--iscc", required=True, type=Path, help="Inno Setup 6.5.4 ISCC.exe")
    parser.add_argument("--output", type=Path, default=ROOT / "dist")
    args = parser.parse_args()
    version = package_version()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="token ashtray setup ") as directory:
        stage = Path(directory) / "payload"
        stage_package(args.package, stage)
        subprocess.run([
            str(args.iscc.resolve()), "/Qp", f"/DPackageDir={stage}",
            f"/DAppVersion={version}", f"/O{output}",
            str(ROOT / "packaging/windows/setup.iss"),
        ], check=True)
    executable = output / "Token-Ashtray-Setup.exe"
    digest = hashlib.sha256(executable.read_bytes()).hexdigest()
    (output / "Token-Ashtray-Setup.exe.sha256").write_text(f"{digest}  {executable.name}\n")
    print(f"{executable}\nSHA-256 {digest}")


if __name__ == "__main__":
    main()
