"""Inspect installer bytes offline and print an UNSIGNED candidate JSON descriptor.

No extraction, execution, network, signature or build-provenance verification.
Source/version/toolchain and reference-resource provenance are caller assertions.
Bounds are mandatory review inputs, not approved production limits. Identical
artifact and inputs yield identical JSON; ZIP generation itself is not promised
reproducible. Only the current Linux setup ELF and the Windows per-user setup EXE
are supported. The Windows setup is inspected beside the validated portable ZIP it
was built from; that build relation is a caller assertion, not extracted.
"""
import argparse
import binascii
from dataclasses import asdict, dataclass
import hashlib
import json
from pathlib import Path
import re
import stat
import struct
import zlib

from package_linux_setup import MAGIC

LINUX = "x86_64-unknown-linux-gnu"
WINDOWS = "x86_64-pc-windows-msvc"
COMMON = {"LICENSE": "LICENSE", "THIRD-PARTY.md": "THIRD-PARTY.md",
          "LICENSE-REVIEW.md": "docs/LICENSE-REVIEW.md"}
# package_release.py runs its CLI on import; packager integration tests keep
# these source paths aligned without changing either existing packager.
REFERENCES = {
    LINUX: {**COMMON, "INSTALL-LINUX.md": "INSTALL-LINUX.md",
            "token-ashtray-256.png": "assets/application/token-ashtray-256.png"},
    WINDOWS: {**COMMON, "INSTALL-WINDOWS.md": "INSTALL-WINDOWS.md",
              "install-windows.ps1": "install-windows.ps1"},
}
APPS = {LINUX: "token-ashtray", WINDOWS: "token-ashtray.exe"}


class Reject(ValueError):
    """The candidate does not match the requested bounded inspection profile."""


def require(condition, message):
    if not condition:
        raise Reject(message)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def canonical(value):
    return (json.dumps(value, sort_keys=True, indent=2, ensure_ascii=True,
                       allow_nan=False) + "\n").encode("ascii")


@dataclass(frozen=True)
class Limits:
    artifact_bytes: int
    member_bytes: int
    expanded_bytes: int
    metadata_bytes: int


def read_bounded(path, limit):
    require(path.is_file() and not path.is_symlink(), "input must be a regular file")
    with path.open("rb") as stream:
        data = stream.read(limit + 1)
    require(0 < len(data) <= limit, "input size exceeds bound or is empty")
    return data


def strict_json(data):
    def pairs(items):
        result = {}
        for key, value in items:
            require(key not in result, "duplicate JSON key")
            result[key] = value
        return result

    def constant(_):
        raise Reject("non-finite JSON number")

    try:
        return json.loads(data, object_pairs_hook=pairs, parse_constant=constant)
    except (ValueError, UnicodeError, RecursionError) as error:
        raise Reject("invalid JSON") from error


def identity(data):
    return {"length": len(data), "sha256": sha(data)}


def pe_optional_header(data, machine, magic, minimum, kind):
    """Check a PE executable's headers and section bounds; return its optional header offset."""
    require(len(data) >= 64 and data[:2] == b"MZ", "expected PE DOS header")
    start = struct.unpack_from("<I", data, 60)[0]
    require(64 <= start <= len(data) - 24 and data[start:start+4] == b"PE\0\0", "invalid PE header")
    found, count, _, _, _, optional_size, flags = struct.unpack_from("<HHIIIHH", data, start + 4)
    optional = start + 24
    sections = optional + optional_size
    require(found == machine and count > 0 and flags & 2 and not flags & 0x2000,
            "expected " + kind + ", not DLL")
    require(optional_size >= minimum and sections + count * 40 <= len(data), "truncated PE headers")
    require(struct.unpack_from("<H", data, optional)[0] == magic, "unexpected PE optional header")
    for offset in range(sections, sections + count * 40, 40):
        size, position = struct.unpack_from("<II", data, offset + 16)
        require(size == 0 or (position >= sections + count * 40 and position + size <= len(data)),
                "truncated PE section")
    return optional


def executable(data, target):
    """Validate executable header/segment bounds, not compiler or runtime support."""
    if target == LINUX:
        require(len(data) >= 64 and data[:7] == b"\x7fELF\x02\x01\x01", "expected little-endian ELF64")
        fields = struct.unpack_from("<16sHHIQQQIHHHHHH", data)
        _, kind, machine, version, _, phoff, _, _, size, phsize, count, _, _, _ = fields
        require(kind in (2, 3) and machine == 62 and version == 1 and size == 64,
                "expected x86_64 ELF executable")
        require(phsize == 56 and count > 0 and phoff >= 64 and phoff + count * 56 <= len(data),
                "invalid ELF program headers")
        loads = 0
        for offset in range(phoff, phoff + count * 56, 56):
            kind, _, start, _, _, filesz, memsz, _ = struct.unpack_from("<IIQQQQQQ", data, offset)
            if kind == 1:
                require(filesz <= memsz and start + filesz <= len(data), "truncated ELF load segment")
                loads += 1
        require(loads > 0, "ELF has no load segment")
        format_name = "ELF64"
    else:
        pe_optional_header(data, 0x8664, 0x20b, 112, "x86_64 PE executable")
        format_name = "PE32+"
    return {"format": format_name, "architecture": "x86_64", "bits": 64,
            "endianness": "little"}


def windows_setup(data):
    """Validate the Inno Setup stub's headers: a 32-bit x86 GUI PE that runs on x64."""
    optional = pe_optional_header(data, 0x14c, 0x10b, 96, "x86 setup executable")
    require(struct.unpack_from("<H", data, optional + 68)[0] == 2, "expected GUI setup")
    return {"format": "PE32", "architecture": "x86", "bits": 32, "endianness": "little",
            "subsystem": "windows-gui"}


def linux_members(data, source_sha, limits):
    end = len(data) - len(MAGIC) - 8
    require(end >= 0 and data.endswith(MAGIC), "missing Linux setup trailer")
    size = struct.unpack_from("<Q", data, end)[0]
    require(0 < size <= min(end, limits.metadata_bytes), "invalid or oversized setup manifest")
    start = end - size
    raw = data[start:end]
    manifest = strict_json(raw)
    require(isinstance(manifest, dict) and set(manifest) == {
        "format", "source_sha", "stub_size", "stub_sha256", "members"}, "unsupported setup manifest")
    require(type(manifest["format"]) is int and manifest["format"] == 1, "unsupported setup format")
    require(manifest["source_sha"] == source_sha, "setup source assertion differs from caller")
    offset = manifest["stub_size"]
    require(type(offset) is int and 0 < offset <= min(start, limits.member_bytes), "invalid stub size")
    stub = data[:offset]
    require(sha(stub) == manifest["stub_sha256"], "setup stub hash mismatch")
    stub_header = executable(stub, LINUX)
    records = manifest["members"]
    require(isinstance(records, list) and len(records) == 7, "expected seven setup members")
    members = {}
    total = 0
    for record in records:
        require(isinstance(record, dict) and set(record) == {"name", "size", "sha256"}, "invalid setup member")
        name, size = record["name"], record["size"]
        require(isinstance(name, str) and name not in members, "duplicate/invalid setup name")
        require(type(size) is int and 0 < size <= limits.member_bytes, "invalid/oversized setup member")
        total += size
        require(total <= limits.expanded_bytes and offset + size <= start, "setup payload bound/truncation")
        body = data[offset:offset+size]
        require(sha(body) == record["sha256"], "setup member hash mismatch")
        members[name] = body
        offset += size
    require(offset == start, "unexpected setup payload bytes")
    return members, {"stub": {**identity(stub), **stub_header},
                     "raw_manifest": {**identity(raw), "offset": start},
                     "raw_trailer": {**identity(data[start:]), "offset": start},
                     "source_sha_assertion": manifest["source_sha"],
                     "mode_origin": "implied_by_current_installer_not_present_in_trailer"}


def windows_members(data, limits):
    """Read the current flat ZIP layout directly; bound DEFLATE's actual output."""
    require(len(data) >= 22, "truncated ZIP")
    end = struct.unpack_from("<4s4H2IH", data, len(data) - 22)
    signature, disk, central_disk, disk_count, count, central_size, central, comment = end
    require(signature == b"PK\x05\x06" and disk == central_disk == comment == 0 and disk_count == count == 7,
            "expected single-disk, comment-free seven-member ZIP")
    require(0 < central_size <= limits.metadata_bytes and central + central_size == len(data) - 22,
            "invalid/oversized ZIP directory")
    offset, local, total = central, 0, 0
    members = {}
    expected = set(REFERENCES[WINDOWS]) | {APPS[WINDOWS], "RELEASE-METRICS.json"}
    for _ in range(count):
        require(offset + 46 <= len(data) - 22, "truncated ZIP directory record")
        record = struct.unpack_from("<4s6H3I5H2I", data, offset)
        (signature, creator, version, flags, compression, clock, date, crc, compressed, size,
         name_size, extra, comment, disk, internal, external, position) = record
        require(signature == b"PK\x01\x02" and creator >> 8 == 3 and version <= 20 and flags == 0,
                "unsupported ZIP record/flags")
        require(extra == comment == disk == internal == 0 and compression in (0, 8), "unsupported ZIP metadata/compression")
        name_end = offset + 46 + name_size
        require(name_size > 0 and name_end <= central + central_size, "truncated ZIP name")
        name_bytes = data[offset+46:name_end]
        try:
            name = name_bytes.decode("ascii")
        except UnicodeError as error:
            raise Reject("non-ASCII ZIP resource name") from error
        require(name in expected and name not in members, "unexpected or duplicate ZIP member")
        require(external == (stat.S_IFREG | 0o644) << 16, "ZIP resource type/mode differs from packager")
        require(0 < size <= limits.member_bytes and 0 < compressed <= limits.artifact_bytes,
                "invalid/oversized ZIP member")
        total += size
        require(total <= limits.expanded_bytes, "ZIP expanded size exceeds bound")
        require(position == local and local + 30 <= central, "ZIP local layout differs from packager")
        header = struct.unpack_from("<4s5H3I2H", data, local)
        require(header == (b"PK\x03\x04", version, flags, compression, clock, date, crc,
                           compressed, size, name_size, 0), "ZIP local/central identity mismatch")
        body_start = local + 30 + name_size
        require(data[local+30:body_start] == name_bytes and body_start + compressed <= central,
                "truncated ZIP member")
        raw = data[body_start:body_start+compressed]
        if compression == 8:
            inflater = zlib.decompressobj(-15)
            try:
                body = inflater.decompress(raw, size + 1)
            except zlib.error as error:
                raise Reject("invalid DEFLATE stream") from error
            require(inflater.eof and not inflater.unused_data and not inflater.unconsumed_tail,
                    "oversized/trailing/truncated DEFLATE output")
        else:
            body = raw
        require(len(body) == size, "actual ZIP output differs from bounded declared size")
        require(binascii.crc32(body) == crc, "ZIP CRC mismatch")
        members[name] = body
        local = body_start + compressed
        offset = name_end
    require(offset == central + central_size and local == central, "unexpected ZIP bytes")
    return members


def inspect_artifact(path, target, version, source_sha, resources, limits, purpose="installer",
                     payload=None):
    """Inspect `path`; a Windows setup also needs `payload`, the ZIP it was built from."""
    require(target in APPS and purpose == "installer", "unsupported target/purpose (installer only)")
    require(isinstance(version, str) and len(version) <= 64 and
            re.fullmatch(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)", version), "expected stable X.Y.Z")
    require(isinstance(source_sha, str) and re.fullmatch(r"[0-9a-f]{40}", source_sha), "expected full lowercase source SHA")
    require(isinstance(limits, Limits) and all(type(n) is int and n > 0 for n in asdict(limits).values()),
            "all caller bounds must be positive integers")
    data = read_bounded(Path(path), limits.artifact_bytes)
    if target == LINUX:
        members, setup = linux_members(data, source_sha, limits)
        kind = "linux-setup-elf-v1"
    else:
        require(payload is not None, "Windows setup needs the portable ZIP it was built from")
        package = read_bounded(Path(payload), limits.artifact_bytes)
        members = windows_members(package, limits)
        setup = {"stub": {**identity(data), **windows_setup(data)},
                 "payload_zip": identity(package),
                 "payload_relation": "caller_asserted_build_input"}
        kind = "windows-setup-exe-v1"
    app_name = APPS[target]
    expected = set(REFERENCES[target]) | {app_name, "RELEASE-METRICS.json"}
    require(set(members) == expected, "package resource set differs from current installer")
    app = members[app_name]
    header = executable(app, target)
    require(len(members["RELEASE-METRICS.json"]) <= limits.metadata_bytes, "oversized metrics JSON")
    metrics = strict_json(members["RELEASE-METRICS.json"])
    require(isinstance(metrics, dict) and metrics.get("binary_sha256") == sha(app) and
            type(metrics.get("binary_bytes")) is int and metrics["binary_bytes"] == len(app),
            "metrics app size/hash mismatch")
    for name, relative in REFERENCES[target].items():
        reference = read_bounded(Path(resources) / relative, limits.member_bytes)
        require(members[name] == reference, "resource differs from supplied reference: " + name)
    result = {"schema": "token-ashtray-unsigned-installer-descriptor-v1", "status": "unsigned-candidate",
              "purpose": purpose, "target": target, "version": version, "source_sha": source_sha,
              "assertions": {"version": "caller_asserted", "source_sha": "caller_asserted",
                             "target_toolchain": "caller_asserted", "compile_provenance": "not_verified",
                             "reference_resource_source_relation": "caller_asserted"},
              "artifact_kind": kind, "artifact": identity(data), "bounds": asdict(limits),
              "app": {"name": app_name, **identity(app), **header},
              "metrics_binding": "app_size_and_sha256_match",
              "reference_resources": "exact_supplied_bytes_match",
              "members": [{"name": name, "type": "regular", "mode": 0o755 if name == app_name and target == LINUX else 0o644,
                           **identity(body)} for name, body in sorted(members.items())]}
    result["linux_setup" if target == LINUX else "windows_setup"] = setup
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifact", type=Path)
    parser.add_argument("--target", choices=APPS, required=True)
    parser.add_argument("--version", required=True, help="caller-asserted stable X.Y.Z, not read from executable")
    parser.add_argument("--source-sha", required=True, help="caller-asserted source commit, not build provenance")
    parser.add_argument("--reference-root", type=Path, required=True,
                        help="reviewed source-resource directory; relation to source SHA is caller asserted")
    parser.add_argument("--payload-zip", type=Path,
                        help="Windows: the validated portable ZIP the setup was built from (caller asserted)")
    for name in ("artifact", "member", "expanded", "metadata"):
        parser.add_argument("--max-" + name + "-bytes", type=int, required=True)
    args = parser.parse_args()
    try:
        result = inspect_artifact(args.artifact, args.target, args.version, args.source_sha,
                                  args.reference_root, Limits(args.max_artifact_bytes, args.max_member_bytes,
                                                             args.max_expanded_bytes, args.max_metadata_bytes),
                                  payload=args.payload_zip)
    except (Reject, OSError, struct.error) as error:
        parser.error(str(error))
    print(canonical(result).decode("ascii"), end="")


if __name__ == "__main__":
    main()
