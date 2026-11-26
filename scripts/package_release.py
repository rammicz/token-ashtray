"""Package one platform's executable, notices, and per-user setup resources."""

import argparse
from pathlib import Path
import shutil
import stat
import zipfile

from path_leak import LeakError, require_clean


parser = argparse.ArgumentParser()
parser.add_argument("binary", type=Path)
parser.add_argument("metrics", type=Path)
parser.add_argument("output", type=Path)
args = parser.parse_args()

common = [
    (args.binary, args.binary.name),
    (Path("LICENSE"), "LICENSE"),
    (Path("THIRD-PARTY.md"), "THIRD-PARTY.md"),
    (Path("docs/LICENSE-REVIEW.md"), "LICENSE-REVIEW.md"),
    (args.metrics, "RELEASE-METRICS.json"),
]
platform_resources = {
    "token-ashtray": [
        (Path("INSTALL-LINUX.md"), "INSTALL-LINUX.md"),
        (Path("install-linux.py"), "install-linux.py"),
        (Path("assets/application/token-ashtray-256.png"), "token-ashtray-256.png"),
    ],
    "token-ashtray.exe": [
        (Path("INSTALL-WINDOWS.md"), "INSTALL-WINDOWS.md"),
        (Path("install-windows.ps1"), "install-windows.ps1"),
    ],
}
if args.binary.name not in platform_resources:
    parser.error("binary must be named token-ashtray or token-ashtray.exe")
members = common + platform_resources[args.binary.name]
for path, _ in members:
    if not path.is_file() or not path.stat().st_size:
        parser.error(f"Missing release input: {path}")
try:
    require_clean({args.binary.name: args.binary.read_bytes()})
except LeakError as error:
    parser.error(str(error))

args.output.parent.mkdir(parents=True, exist_ok=True)
with zipfile.ZipFile(args.output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
    for path, name in members:
        info = zipfile.ZipInfo.from_file(path, name)
        # Target permissions must not depend on the packaging host's filesystem.
        mode = 0o755 if name in {"token-ashtray", "install-linux.py"} else 0o644
        info.create_system = 3  # Unix metadata, including on Windows build hosts.
        info.external_attr = (stat.S_IFREG | mode) << 16
        info.compress_type = zipfile.ZIP_DEFLATED
        with path.open("rb") as source, archive.open(info, "w") as target:
            shutil.copyfileobj(source, target)
print(args.output)
