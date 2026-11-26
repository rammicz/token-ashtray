"""Append a strictly allowlisted payload to the native Linux setup ELF."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import tempfile

from path_leak import LeakError, require_clean

MAGIC = b"TOKEN-ASHTRAY-SETUP-v1\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("setup", type=Path)
    parser.add_argument("binary", type=Path)
    parser.add_argument("metrics", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--source-sha", required=True)
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", args.source_sha):
        parser.error("source SHA must be a full lowercase Git commit identity")
    sources = [(args.binary, "token-ashtray"),
               (Path("assets/application/token-ashtray-256.png"), "token-ashtray-256.png"),
               (Path("LICENSE"), "LICENSE"), (Path("THIRD-PARTY.md"), "THIRD-PARTY.md"),
               (Path("docs/LICENSE-REVIEW.md"), "LICENSE-REVIEW.md"),
               (Path("INSTALL-LINUX.md"), "INSTALL-LINUX.md"),
               (args.metrics, "RELEASE-METRICS.json")]
    for path in [args.setup] + [path for path, _ in sources]:
        if not path.is_file() or not path.stat().st_size:
            parser.error(f"Missing release input: {path}")
    stub = args.setup.read_bytes()
    bodies = [(name, path.read_bytes()) for path, name in sources]
    if not stub.startswith(b"\x7fELF") or not bodies[0][1].startswith(b"\x7fELF"):
        parser.error("setup and app must be Linux ELF executables")
    if stub.endswith(MAGIC):
        parser.error("setup input already contains a payload")
    try:
        require_clean({args.setup.name: stub, args.binary.name: bodies[0][1]})
    except LeakError as error:
        parser.error(str(error))
    app_hash = hashlib.sha256(bodies[0][1]).hexdigest()
    try:
        metrics = json.loads(args.metrics.read_text())
    except (ValueError, UnicodeError) as error:
        parser.error(f"Invalid release metrics: {error}")
    if metrics.get("binary_sha256") != app_hash:
        parser.error("release metrics SHA-256 does not match approved app")
    manifest = json.dumps({"format": 1, "source_sha": args.source_sha,
                           "stub_size": len(stub), "stub_sha256": hashlib.sha256(stub).hexdigest(),
                           "members": [{"name": name, "size": len(body),
                                        "sha256": hashlib.sha256(body).hexdigest()}
                                       for name, body in bodies]}, separators=(",", ":")).encode()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=args.output.parent, prefix=".setup-", delete=False) as file:
            temporary = Path(file.name)
            file.write(stub)
            for _, body in bodies:
                file.write(body)
            file.write(manifest)
            file.write(struct.pack("<Q", len(manifest)))
            file.write(MAGIC)
            file.flush()
            os.fsync(file.fileno())
        temporary.chmod(0o755)
        os.replace(temporary, args.output)
        temporary = None
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    print(args.output)


if __name__ == "__main__":
    main()
