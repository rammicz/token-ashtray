"""Export the public source tree from one git ref and scan it for private data.

    python3 scripts/public_export.py REF TARGET   # export REF into an empty TARGET, then scan
    python3 scripts/public_export.py --scan TREE  # scan an existing tree only

The export keeps the committed blob bytes (no line-ending conversion) and omits
internal records: QA evidence folders, a verifier session report, agent
tooling, agent instructions, the historical distribution proposal and scratch
files. The scan then checks every
exported file, as raw bytes and as UTF-16LE text, for credentials, personal
e-mail addresses, this host's git identity and local paths, user profile
paths, internal host names, private tracker links, internal queue markers,
conversation share links, image metadata chunks and Markdown links to files the export lacks.

Reports carry fixed labels, exported relative paths and counts only, never the
matched text, so no account or host name is printed. Host names are held as
SHA-256 digests for the same reason. The exit status is 1 when anything is
found and 2 when the export itself fails.
"""

import argparse
from collections import Counter
import hashlib
import io
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
import tarfile
from urllib.parse import unquote

from path_leak import local_paths, local_pattern, views

ROOT = Path(__file__).resolve().parents[1]

# Matched against tree paths from the start; directories end in "/".
EXCLUDED = [re.compile(pattern) for pattern in (
    r"docs/[^/]+-evidence/",
    r"\.vibe/",
    r"scratch/",
    r"AGENTS\.md$",
    r"docs/updates/draft-proposal\.md$",
    # A verifier's session report; no test reads it.
    r"tests/fixtures/updates/offline_prototype/evidence/independent-verification\.md$",
)]
# Required upstream notices legitimately name their authors, and upstream
# documents link files that were not vendored.
THIRD_PARTY = re.compile(r"vendor/|THIRD-PARTY\.md$")

# Views are lowercased, so patterns are lowercase. Character classes keep this
# file's own pattern text from matching itself when the export is scanned.
CONTENT = [
    ("credential", re.compile(rb"\bgh[opusr]_[a-z0-9]{36}\b")),
    ("credential", re.compile(rb"github_pat_[a-z0-9_]{22,}")),
    ("credential", re.compile(rb"\bakia[0-9a-z]{16}\b")),
    ("credential", re.compile(rb"\bxox[abprs]-[a-z0-9-]{10,}")),
    ("credential", re.compile(rb"\bsk-[a-z0-9_-]{20,}")),
    ("credential", re.compile(rb"\baiza[0-9a-z_-]{35}\b")),
    ("credential", re.compile(rb"eyj[a-z0-9_-]{10,}\.eyj[a-z0-9_-]{10,}\.")),
    ("credential", re.compile(rb"-----begin [a-z ]*private key-----")),
    ("private-issue-url", re.compile(rb"github\.com/rammicz/token-ashtray/(?:issues|pull|discussions)\b")),
    ("private-issue-url", re.compile(rb"token-ashtray-privat[e]")),
    ("internal-marker", re.compile(rb"\bqueue[:](?:dev|windowsdev|admin)\b")),
    # A shared conversation page can show the account that shared it.
    ("conversation-share-link", re.compile(rb"\b(?:chatgpt\.com|chat\.openai\.com)/(?:s|share)/"
                                           rb"|\bclaude\.ai/share/|\bg\.co/gemini/share/")),
    ("user-profile-path", re.compile(rb"[a-z]:[\\/]+users[\\/]+(?P<user>[^\\/\x00-\x1f\"'`<>|*?%$]{1,64}?)[\\/]")),
    ("user-profile-path", re.compile(rb"(?<![\w.-])/home/(?P<user>[^/\x00-\x20\"'`<>$]{1,64})/")),
]
PERSONAL_EMAIL = re.compile(
    rb"[a-z0-9._%+-]+@(?:gmail|googlemail|seznam|email|centrum|post|volny|atlas|outlook|hotmail"
    rb"|live|yahoo|icloud|me|proton|protonmail|pm|tutanota|gmx|yandex)(?:\.[a-z]{2,6}){1,2}\b")
# Documentation and test placeholders for user profile folders.
PLACEHOLDER_USERS = {b"a", b"john", b"user", b"username", b"you", b"example", b"builder",
                     b"public", b"default", b"runner", b"runneradmin", b"qa user"}
# SHA-256 of lowercased internal host names; the names stay out of the public tree.
INTERNAL_HOSTS = {
    "f28aac943ea1c6b3236201bb6b76e3069536416776ad2699babdb968cc93fd44",
    "b170f30d201da73b7637e94391f22a4642f8c396f9c1c9f8b2799919ced46d78",
    "14934e9c0a7b9c9df53bb814cf521e63fbdaeab37427961cbb7eebedab9d97b1",
    "aa31a244c194d3812128b9c2e6ec61fd6465d60f22d9380a12a5b2e17cfba4f1",
}
WORD = re.compile(rb"[a-z0-9]+")
PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
# Rendering chunks; text, EXIF, provenance and unknown chunks are metadata.
PNG_RENDERING = {b"IHDR", b"PLTE", b"IDAT", b"IEND", b"tRNS", b"gAMA", b"cHRM", b"sRGB", b"iCCP",
                 b"sBIT", b"bKGD", b"pHYs", b"hIST", b"sPLT", b"tIME", b"cICP", b"acTL", b"fcTL",
                 b"fdAT"}
SVG_METADATA = re.compile(rb"<metadata|<rdf:|\bsodipodi:|\binkscape:")
MARKDOWN_LINK = re.compile(r"\]\(\s*<?([^)\s>]+)|^\s{0,3}\[[^\]]+\]:\s*<?(\S+?)>?(?:\s|$)", re.M)
SCHEME = re.compile(r"[a-z][a-z0-9+.-]*:", re.I)


class ExportError(RuntimeError):
    """The export could not be produced."""


def excluded(path):
    return any(pattern.match(path) for pattern in EXCLUDED)


def export(ref, target, repository=ROOT):
    """Write REF's tree minus EXCLUDED into an empty TARGET; return (kept, omitted) file counts."""
    target = Path(target)
    if target.exists() and any(target.iterdir()):
        raise ExportError("target directory is not empty")
    archive = subprocess.run(["git", "-c", "core.autocrlf=false", "archive", "--format=tar", ref],
                             cwd=repository, capture_output=True, check=False)
    if archive.returncode:
        raise ExportError("git archive failed: " + archive.stderr.decode(errors="replace").strip())
    target.mkdir(parents=True, exist_ok=True)
    with tarfile.open(fileobj=io.BytesIO(archive.stdout)) as tar:
        members = tar.getmembers()
        kept = [member for member in members
                if not excluded(member.name.rstrip("/") + ("/" if member.isdir() else ""))]
        options = {"filter": "data"} if hasattr(tarfile, "data_filter") else {}
        tar.extractall(target, members=kept, **options)
    files = sum(1 for member in kept if member.isfile())
    return files, sum(1 for member in members if member.isfile()) - files


def png_metadata(data):
    """Count non-rendering chunks in every PNG stream, including PNGs inside ICO files."""
    count = 0
    start = data.find(PNG_SIGNATURE)
    while start >= 0:
        offset = start + len(PNG_SIGNATURE)
        while offset + 8 <= len(data):
            length, kind = struct.unpack(">I4s", data[offset:offset + 8])
            count += kind not in PNG_RENDERING
            offset += 12 + length
            if kind == b"IEND":
                break
        start = data.find(PNG_SIGNATURE, offset)
    return count


def dangling_links(text, source, root):
    """Count relative Markdown links whose target file is not in the tree."""
    count = 0
    fenced = False
    for line in text.splitlines():
        if line.lstrip().startswith(("```", "~~~")):
            fenced = not fenced
            continue
        if fenced:
            continue
        for match in MARKDOWN_LINK.finditer(line):
            link = (match.group(1) or match.group(2)).split("#")[0].split("?")[0]
            if not link or SCHEME.match(link):
                continue
            base = root if link.startswith("/") else source.parent
            count += not (base / unquote(link).lstrip("/")).exists()
    return count


class Scanner:
    def __init__(self, root):
        self.root = Path(root)
        self.hosts = {}
        identity = subprocess.run(["git", "config", "user.email"], cwd=ROOT, capture_output=True,
                                  text=True, check=False).stdout.strip().lower()
        self.identity = identity.encode() if "@" in identity else None
        # Host-specific locations, including the checkout's parent folder.
        self.local = [local_pattern(value) for value in local_paths([ROOT.parent, Path.home()]).values()]

    def host_words(self, view):
        found = 0
        for word, count in Counter(WORD.findall(view)).items():
            if word not in self.hosts:
                self.hosts[word] = hashlib.sha256(word).hexdigest() in INTERNAL_HOSTS
            found += self.hosts[word] * count
        return found

    def scan_file(self, relative, data):
        counts = Counter()
        third_party = THIRD_PARTY.match(relative)
        for _, view in views(data):
            for label, pattern in CONTENT:
                for match in pattern.finditer(view):
                    if "user" in pattern.groupindex and match.group("user") in PLACEHOLDER_USERS:
                        continue
                    counts[label] += 1
            if not third_party:
                counts["personal-email"] += len(PERSONAL_EMAIL.findall(view))
            if self.identity:
                counts["local-git-identity"] += view.count(self.identity)
            counts["local-path"] += sum(len(pattern.findall(view)) for pattern in self.local)
            counts["internal-host"] += self.host_words(view)
        counts["image-metadata"] += png_metadata(data)
        if relative.endswith(".svg"):
            counts["image-metadata"] += len(SVG_METADATA.findall(data.lower()))
        if relative.endswith(".md") and not third_party:
            counts["dangling-link"] += dangling_links(data.decode("utf-8", "replace"),
                                                      self.root / relative, self.root)
        return +counts

    def scan(self):
        """Map (label, relative path) to a count for the whole tree."""
        findings = {}
        for directory, folders, files in os.walk(self.root):
            folders[:] = sorted(folder for folder in folders if folder != ".git")
            for name in sorted(files):
                path = Path(directory, name)
                relative = path.relative_to(self.root).as_posix()
                for label, count in self.scan_file(relative, path.read_bytes()).items():
                    findings[label, relative] = count
        return findings


def report(findings):
    if not findings:
        return "public_export: privacy scan clean (0 findings)"
    totals = Counter()
    for (label, _), count in findings.items():
        totals[label] += count
    lines = [f"public_export: privacy scan found {sum(totals.values())} finding(s)"]
    previous = None
    for (label, path), count in sorted(findings.items()):
        if label != previous:
            lines.append(f"  {label}: {totals[label]}")
            previous = label
        lines.append(f"    {path} x{count}")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--scan", action="store_true", help="scan TREE without exporting")
    parser.add_argument("paths", nargs="+", metavar="REF TARGET | TREE")
    args = parser.parse_args()
    if args.scan != (len(args.paths) == 1):
        parser.error("give REF and TARGET, or --scan and one TREE")
    tree = Path(args.paths[-1])
    if not args.scan:
        try:
            kept, omitted = export(args.paths[0], tree)
        except ExportError as error:
            print(f"public_export: {error}", file=sys.stderr)
            return 2
        print(f"public_export: exported {kept} file(s), omitted {omitted} internal file(s)")
    findings = Scanner(tree).scan()
    print(report(findings))
    return 1 if findings else 0


if __name__ == "__main__":
    sys.exit(main())
