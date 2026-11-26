"""Refuse release executables that embed build-host paths.

Release builds remap host paths (see scripts/release_build.py), so a shipped
executable keeps only toolchain paths such as /rustc/<commit>/library/...,
the remap tokens ~, /cargo, /rustup, /token-ashtray and /target, and relative
vendor/ paths. Anything resembling a home directory, a Cargo or rustup home,
a CI checkout or this machine's actual local paths is refused. The patterns
are checked in raw bytes and in UTF-16LE strings. Reports carry counts and
fixed redacted labels only, never the matched text, so no account name is
printed.
"""

import argparse
import os
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
# No lookbehinds: Rust packs string literals without separators, and in UTF-16
# text an adjacent string's last character can precede the drive letter. Each
# pattern ends in a separator, so URLs such as https://github.com/ do not match.
DRIVE = rb"[a-z]:[\\/]+"
SEGMENT = rb"[^\\/\x00-\x20]+"
# Account names may contain spaces; a trailing / bounds the match.
HOME_SEGMENT = rb"[^\\/\x00-\x1f]{1,64}"
# Views are lowercased once, so patterns are lowercase without re.I (much faster).
# Labels are fixed and redacted: matched text is never reported.
PATTERNS = [
    (r"windows-profile [X:\Users\<user>]", re.compile(DRIVE + rb"users[\\/]+" + SEGMENT)),
    ("unix-home [/home/<user>/]", re.compile(rb"/home/" + HOME_SEGMENT + rb"/")),
    ("macos-home [/Users/<user>/]", re.compile(rb"(?<!:)/users/" + HOME_SEGMENT + rb"/")),
    ("root-home [/root/]", re.compile(rb"/root/")),
    ("cargo-home [.cargo/{registry,git}]", re.compile(rb"\.cargo[\\/]+(?:registry|git)")),
    ("rustup-home [.rustup/]", re.compile(rb"\.rustup[\\/]")),
    (r"ci-checkout [X:\{a,github}\]", re.compile(DRIVE + rb"(?:a|github)[\\/]")),
]
# Keeps a UTF-16LE code unit's low byte only where its high byte is zero.
ZERO_HIGH_BYTE = bytes([0xFF] + [0] * 255)
LOCAL_VARIABLES = ("HOME", "USERPROFILE", "CARGO_HOME", "RUSTUP_HOME", "CARGO_TARGET_DIR",
                   "TEMP", "TMP", "TMPDIR")


class LeakError(ValueError):
    """An executable contains build-host paths."""


def components(value):
    return re.split(r"[\\/]+", value.rstrip("\\/"))


def local_paths(extra=()):
    """This host's private locations; shallow roots such as /tmp or C:\\ are left to PATTERNS."""
    found = {"repository root": str(ROOT)}
    for name in LOCAL_VARIABLES:
        if os.environ.get(name):
            found[name] = os.environ[name]
    for index, path in enumerate(extra):
        found[f"build path {index + 1}"] = str(path)
    return {label: value for label, value in found.items() if len(list(filter(None, components(value)))) >= 2}


def local_pattern(value):
    parts = [re.escape(part.lower().encode()) for part in components(value)]
    return re.compile(rb"[\\/]+".join(parts) + rb"(?![\w.-])")


def utf16_view(data, offset):
    """Code units at one alignment as single bytes; wide characters become NUL separators."""
    highs = data[offset + 1::2]
    lows = data[offset::2][:len(highs)]
    keep = int.from_bytes(highs.translate(ZERO_HIGH_BYTE), "big")
    return (int.from_bytes(lows, "big") & keep).to_bytes(len(highs), "big")


def views(data):
    """Raw bytes, then UTF-16LE text at both byte alignments, lowercased.

    Plain ASCII never survives the projection, since every other byte of it is
    nonzero, so each string is counted in exactly one view.
    """
    yield "", data.lower()
    yield " utf-16le", (utf16_view(data, 0) + b"\0" + utf16_view(data, 1)).lower()


def find_leaks(data, patterns):
    """Map a redacted description to its match count."""
    counts = {}
    for encoding, view in views(data):
        for label, pattern in patterns:
            count = sum(1 for _ in pattern.finditer(view))
            if count:
                counts[label + encoding] = count
    return counts


def require_clean(bodies, extra_local=()):
    """Raise LeakError with each named body's redacted findings, before anything is packaged."""
    patterns = PATTERNS + [(f"local-path [<{label}>]", local_pattern(value))
                           for label, value in local_paths(extra_local).items()]
    problems = []
    for name, data in bodies.items():
        counts = find_leaks(data, patterns)
        if counts:
            details = ", ".join(f"{label} x{count}" for label, count in counts.items())
            problems.append(f"{name}: {details}")
    if problems:
        raise LeakError("build-host paths found; rebuild with scripts/release_build.py: "
                        + "; ".join(problems))


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("files", nargs="+", type=Path)
    args = parser.parse_args()
    try:
        require_clean({path.name: path.read_bytes() for path in args.files})
    except LeakError as error:
        sys.exit(f"path_leak: {error}")
    print(f"path_leak: no build-host paths in {len(args.files)} file(s)")


if __name__ == "__main__":
    main()
