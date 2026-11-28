"""Public export: omitted internal records, exact bytes and a redacting privacy scan."""
from contextlib import contextmanager
import getpass
import hashlib
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
import zlib

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/public_export.py"
sys.path.insert(0, str(ROOT / "scripts"))
import public_export  # noqa: E402

# Samples are assembled from fragments so this file itself scans clean.
HOST = "examplehost"
NOREPLY = "12345+fixture" + "@" + "users.noreply.github.com"
LEGACY_NOREPLY = "fixture-user" + "@" + "users.noreply.github.com"
SAMPLES = {
    "credential": "token = " + "ghp" + "_" + "a1" * 18,
    "personal-email": "write to jan.novak" + "@" + "gmail.com",
    "private-issue-url": "see https://github.com/rammicz/token-ashtray/" + "issues/7",
    "internal-marker": "routed to queue" + ":windowsdev",
    "conversation-share-link": "https://chatgpt.com/" + "s/abc123",
    "user-profile-path": "C:" + "\\Users\\jnovak\\project",
    "internal-host": "ssh build@" + HOST.upper() + " now",
}


def png(*chunks):
    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))
    header = struct.pack(">IIBBBBB", 1, 1, 8, 6, 0, 0, 0)
    pixels = zlib.compress(b"\0\0\0\0\0")
    return (public_export.PNG_SIGNATURE + chunk(b"IHDR", header)
            + b"".join(chunk(kind, body) for kind, body in chunks)
            + chunk(b"IDAT", pixels) + chunk(b"IEND", b""))


class Export(unittest.TestCase):
    def repository(self, root):
        files = {
            "README.md": b"# App\n\nSee [the guide](docs/GUIDE.md).\n",
            "docs/GUIDE.md": b"Guide\r\nwith CRLF kept as committed\n",
            "docs/UI-25-evidence/README.md": b"internal\n",
            "docs/updates/draft-proposal.md": b"historical\n",
            "docs/updates/contract.md": b"kept\n",
            ".vibe/skills/SKILL.md": b"agent tooling\n",
            "AGENTS.md": b"agent instructions\n",
            "src/AGENTS.md": b"not the root file\n",
            "tests/fixtures/updates/offline_prototype/evidence/independent-verification.md": b"report\n",
            "tests/fixtures/updates/offline_prototype/evidence/expected-vectors.json": b"[]\n",
        }
        for name, body in files.items():
            (root / name).parent.mkdir(parents=True, exist_ok=True)
            (root / name).write_bytes(body)
        git = ["git", "-c", "core.autocrlf=false", "-c", "user.name=Fixture",
               "-c", "user.email=fixture@example.invalid"]
        subprocess.run(["git", "init", "-q"], cwd=root, check=True)
        subprocess.run(git + ["add", "-A"], cwd=root, check=True)
        subprocess.run(git + ["commit", "-q", "-m", "fixture"], cwd=root, check=True)
        return files

    def test_export_omits_internal_records_and_keeps_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            source, target = Path(directory, "source"), Path(directory, "public")
            source.mkdir()
            files = self.repository(source)
            kept, omitted = public_export.export("HEAD", target, repository=source)
            exported = sorted(path.relative_to(target).as_posix()
                              for path in target.rglob("*") if path.is_file())
            self.assertEqual(exported, ["README.md", "docs/GUIDE.md", "docs/updates/contract.md",
                                        "src/AGENTS.md",
                                        "tests/fixtures/updates/offline_prototype/evidence/expected-vectors.json"])
            self.assertEqual((kept, omitted), (5, 5))
            for name in exported:
                self.assertEqual((target / name).read_bytes(), files[name])

    def test_export_refuses_a_non_empty_target(self):
        with tempfile.TemporaryDirectory() as directory:
            source, target = Path(directory, "source"), Path(directory, "public")
            source.mkdir()
            self.repository(source)
            target.mkdir()
            (target / "keep.txt").write_text("existing")
            with self.assertRaises(public_export.ExportError):
                public_export.export("HEAD", target, repository=source)
            self.assertEqual([path.name for path in target.iterdir()], ["keep.txt"])


class Scan(unittest.TestCase):
    def scan(self, files, identity=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, body in files.items():
                (root / name).parent.mkdir(parents=True, exist_ok=True)
                (root / name).write_bytes(body if isinstance(body, bytes) else body.encode())
            scanner = public_export.Scanner(root)
            scanner.identity = identity
            hosts = {hashlib.sha256(HOST.encode()).hexdigest()}
            with mock.patch.object(public_export, "INTERNAL_HOSTS", hosts):
                return scanner.scan()

    def test_each_category_is_counted_per_file(self):
        files = {f"{label}.txt": sample for label, sample in SAMPLES.items()}
        findings = self.scan(files)
        self.assertEqual(findings, {(label, f"{label}.txt"): 1 for label in SAMPLES})

    def test_desktop_host_named_after_the_account_is_refused(self):
        # Derived at run time, so the name itself never appears in the source.
        name = getpass.getuser().lower() + "buntu"
        if hashlib.sha256(name.encode()).hexdigest() not in public_export.INTERNAL_HOSTS:
            self.skipTest("only the account that names the desktop host can plant it")
        with tempfile.TemporaryDirectory() as directory:
            Path(directory, "note.md").write_text(f"Ran on {name.capitalize()} desktop.\n")
            findings = public_export.Scanner(directory).scan()
        self.assertEqual(findings, {("internal-host", "note.md"): 1})

    def test_free_mail_on_other_country_domains_is_found(self):
        findings = self.scan({"a.txt": "jan" + "@" + "proton.ch", "b.txt": "jan" + "@" + "yahoo.co.uk"})
        self.assertEqual(findings, {("personal-email", "a.txt"): 1, ("personal-email", "b.txt"): 1})

    def test_utf16_text_is_scanned(self):
        findings = self.scan({"resource.bin": SAMPLES["credential"].encode("utf-16-le")})
        self.assertEqual(findings, {("credential", "resource.bin"): 1})

    def test_local_identity_and_paths_are_found_without_being_stored_here(self):
        findings = self.scan({"notes.md": f"by someone@example.org in {Path.home() / 'src'}\n"},
                             identity=b"someone@example.org")
        self.assertEqual(findings.get(("local-git-identity", "notes.md")), 1)
        self.assertGreaterEqual(findings.get(("local-path", "notes.md"), 0), 1)

    def test_placeholders_notices_and_public_links_are_clean(self):
        files = {
            "INSTALL.md": "Logs live in C:\\Users\\<user>\\AppData and C:\\Users\\QA User\\x and "
                          "/home/john/.config; see https://github.com/rammicz/token-ashtray/releases\n",
            "THIRD-PARTY.md": "Copyright upstream author <" + "dev" + "@" + "gmail.com>\n",
            "vendor/lib/README.md": "Upstream [file](missing.md)\n",
            "docs/a.md": "[b](b.md#part) [web](https://example.org/x.md) [top](#top) [mail](mailto:x)\n"
                         "```\n[not a link](absent.md)\n```\n[ref]: b.md\n",
            "docs/b.md": "fine\n",
            "icon.png": png((b"pHYs", bytes(9)), (b"tIME", bytes(7))),
            "art.svg": '<svg xmlns="http://www.w3.org/2000/svg"><path d="M0 0"/></svg>',
        }
        self.assertEqual(self.scan(files), {})

    def test_metadata_and_dangling_links_are_found(self):
        files = {
            "art.png": png((b"tEXt", b"Author\0someone"), (b"caBX", b"provenance")),
            "sheet.ico": b"\0\0\1\0" + png((b"iTXt", b"x")),
            "drawing.svg": '<svg><metadata><rdf:RDF/></metadata></svg>',
            "docs/index.md": "[gone](evidence/README.md) and [ok](index.md)\n[ref]: ../AGENTS.md\n",
        }
        self.assertEqual(self.scan(files), {
            ("image-metadata", "art.png"): 2,
            ("image-metadata", "sheet.ico"): 1,
            ("image-metadata", "drawing.svg"): 2,
            ("dangling-link", "docs/index.md"): 2,
        })


class CommandLine(unittest.TestCase):
    def run_scan(self, root):
        return subprocess.run([sys.executable, str(SCRIPT), "--scan", str(root)],
                              capture_output=True, text=True, check=False)

    def test_findings_fail_with_counts_only(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            secrets = [SAMPLES[label] for label in ("credential", "personal-email", "user-profile-path")]
            (root / "leak.txt").write_text("\n".join(secrets))
            result = self.run_scan(root)
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn("leak.txt x1", result.stdout)
            for secret in secrets:
                for fragment in secret.split():
                    if len(fragment) > 6:
                        self.assertNotIn(fragment, result.stdout)
            self.assertNotIn("jnovak", result.stdout)

    def test_clean_tree_passes(self):
        with tempfile.TemporaryDirectory() as directory:
            Path(directory, "README.md").write_text("# Clean\n")
            result = self.run_scan(directory)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("0 findings", result.stdout)

    def test_usage_requires_ref_and_target_or_one_tree(self):
        result = subprocess.run([sys.executable, str(SCRIPT), "HEAD"], capture_output=True,
                                text=True, check=False)
        self.assertEqual(result.returncode, 2)

    def test_usage_rejects_conflicting_or_missing_check_git_args(self):
        for extra in (["--scan", "--check-git", "HEAD"], ["--check-git"]):
            result = subprocess.run([sys.executable, str(SCRIPT), *extra], capture_output=True,
                                    text=True, check=False)
            self.assertEqual(result.returncode, 2, extra)

    def test_check_git_cli_fails_with_redacted_counts_only(self):
        with GitAttribution.temp_repo() as source:
            author = "personal.user" + "@" + "example.invalid"
            tagger = "tagger.secret" + "@" + "private.invalid"
            GitAttribution.commit(source, "README.md", "# Demo\n", author, author)
            GitAttribution.annotated_tag(source, "v0.0.1", "release tag", tagger)
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--repo", str(source),
                 "--check-git", "HEAD", "refs/tags/v0.0.1"],
                capture_output=True, text=True, check=False,
            )
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn("commit-author-email: 1", result.stdout)
            self.assertIn("commit-committer-email: 1", result.stdout)
            self.assertIn("tagger-email: 1", result.stdout)
            combined = result.stdout + result.stderr
            for leaked in ("personal.user", "example.invalid", "tagger.secret", "private.invalid"):
                self.assertNotIn(leaked, combined)

    def test_check_git_cli_passes_on_clean_repo(self):
        with GitAttribution.temp_repo() as source:
            GitAttribution.commit(source, "README.md", "# Clean\n", NOREPLY, NOREPLY)
            subprocess.run(["git", "tag", "v0.1.0"], cwd=source, check=True)
            GitAttribution.annotated_tag(source, "v0.2.0", "clean annotated tag", LEGACY_NOREPLY)
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--repo", str(source),
                 "--check-git", "HEAD", "refs/tags/v0.1.0", "refs/tags/v0.2.0"],
                capture_output=True, text=True, check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("0 findings", result.stdout)

    def test_check_git_cli_rejects_unknown_ref(self):
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--check-git", "refs/heads/does-not-exist-test-ref"],
            capture_output=True, text=True, check=False,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("unknown git ref", result.stderr)


class GitAttribution(unittest.TestCase):
    @staticmethod
    @contextmanager
    def temp_repo():
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory, "source")
            source.mkdir()
            subprocess.run(["git", "init", "-q"], cwd=source, check=True)
            yield source

    @staticmethod
    def commit(root, name, content, author_email, committer_email):
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
        subprocess.run(["git", "add", name], cwd=root, check=True)
        env = {
            **os.environ,
            "GIT_AUTHOR_NAME": "Fixture Author",
            "GIT_AUTHOR_EMAIL": author_email,
            "GIT_COMMITTER_NAME": "Fixture Committer",
            "GIT_COMMITTER_EMAIL": committer_email,
        }
        subprocess.run(["git", "-c", "commit.gpgsign=false", "commit", "-q", "-m", f"add {name}"],
                       cwd=root, env=env, check=True)

    @staticmethod
    def annotated_tag(root, tag_name, message, tagger_email, target="HEAD"):
        env = {
            **os.environ,
            "GIT_COMMITTER_NAME": "Fixture Tagger",
            "GIT_COMMITTER_EMAIL": tagger_email,
        }
        subprocess.run(
            ["git", "-c", "advice.nestedTag=false", "-c", "tag.gpgSign=false",
             "tag", "-a", tag_name, target, "-m", message],
            cwd=root, env=env, check=True,
        )

    def test_non_approved_commit_and_tag_emails_are_counted_once(self):
        with self.temp_repo() as source:
            author = "personal.user" + "@" + "example.invalid"
            committer = "corp.builder" + "@" + "example.org"
            tagger = "tagger.person" + "@" + "example.net"
            self.commit(source, "a.txt", "hello\n", author, committer)
            self.annotated_tag(source, "v0.0.1", "annotated", tagger)
            findings = public_export.check_git_refs(["HEAD", "refs/tags/v0.0.1"], repository=source)
            self.assertEqual(findings, {
                "commit-author-email": 1,
                "commit-committer-email": 1,
                "tagger-email": 1,
            })

    def test_mixed_commits_count_only_non_approved_identities(self):
        with self.temp_repo() as source:
            bot = "41898282+github-actions[bot]" + "@" + "users.noreply.github.com"
            self.commit(source, "a.txt", "1\n", NOREPLY, "builder" + "@" + "example.invalid")
            self.commit(source, "b.txt", "2\n", "author" + "@" + "example.invalid", LEGACY_NOREPLY)
            self.commit(source, "c.txt", "3\n", bot, bot)
            findings = public_export.check_git_refs(["HEAD"], repository=source)
            self.assertEqual(findings, {
                "commit-author-email": 1,
                "commit-committer-email": 1,
            })

    def test_clean_noreply_commits_lightweight_and_annotated_tags_pass(self):
        with self.temp_repo() as source:
            self.commit(source, "a.txt", "hello\n", NOREPLY, NOREPLY)
            subprocess.run(["git", "tag", "v0.1.0"], cwd=source, check=True)
            decoy = "release\n\ntagger Fake <bad" + "@" + "example.invalid> 1700000000 +0000"
            self.annotated_tag(source, "v0.2.0", decoy, LEGACY_NOREPLY)
            findings = public_export.check_git_refs(
                ["HEAD", "refs/tags/v0.1.0", "refs/tags/v0.2.0"], repository=source
            )
            self.assertEqual(findings, {})

    def test_nested_annotated_tag_checks_every_tag_object(self):
        with self.temp_repo() as source:
            bad = "nested.tagger" + "@" + "example.invalid"
            self.commit(source, "a.txt", "hello\n", NOREPLY, NOREPLY)
            self.annotated_tag(source, "v0.1.0", "inner", bad)
            self.annotated_tag(source, "v0.1.0-wrap", "outer", NOREPLY, target="refs/tags/v0.1.0")
            findings = public_export.check_git_refs(["refs/tags/v0.1.0-wrap"], repository=source)
            self.assertEqual(findings, {"tagger-email": 1})

    def test_invalid_empty_and_non_commit_refs_raise_export_error(self):
        with self.temp_repo() as source:
            self.commit(source, "a.txt", "hello\n", NOREPLY, NOREPLY)
            for bad_refs in ([], ["refs/heads/nonexistent"], ["HEAD^{tree}"]):
                with self.assertRaises(public_export.ExportError, msg=bad_refs):
                    public_export.check_git_refs(bad_refs, repository=source)

    def test_approved_git_email_validation(self):
        approved = [
            NOREPLY,
            "99999+Example-User" + "@" + "USERS.NOREPLY.GITHUB.COM",
            LEGACY_NOREPLY,
            "a" + "@" + "users.noreply.github.com",
            "41898282+github-actions[bot]" + "@" + "users.noreply.github.com",
        ]
        rejected = [
            "",
            "personal" + "@" + "example.invalid",
            "12345+user" + "@" + "users.noreply.github.com.evil.com",
            "abc+user" + "@" + "users.noreply.github.com",
            "+user" + "@" + "users.noreply.github.com",
            "12345+" + "@" + "users.noreply.github.com",
            "user.name" + "@" + "users.noreply.github.com",
            "-user" + "@" + "users.noreply.github.com",
            "user-" + "@" + "users.noreply.github.com",
        ]
        for email in approved:
            self.assertTrue(public_export.approved_git_email(email), email)
        for email in rejected:
            self.assertFalse(public_export.approved_git_email(email), email)

    def test_live_repository_head_tree_and_history_are_clean(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory, "public")
            public_export.export("HEAD", target)
            self.assertEqual(public_export.Scanner(target).scan(), {})
        self.assertEqual(public_export.check_git_refs(["HEAD"]), {})


if __name__ == "__main__":
    unittest.main()
