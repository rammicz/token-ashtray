"""Build release executables without embedding build-host paths.

This is the only supported build for packaged executables. It runs
`cargo build --release --locked` with rustc path remapping in
CARGO_ENCODED_RUSTFLAGS: the home directory becomes ~, the Cargo and rustup
homes /cargo and /rustup, this checkout /token-ashtray and the Cargo output
directory /target. An MSVC target also links the C runtime statically. Each
built executable is then scanned with scripts/path_leak.py and refused if a
build-host path remains.
"""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys

from path_leak import LeakError, require_clean

ROOT = Path(__file__).resolve().parents[1]

# Any of these would replace or be replaced by the remap flags, never merge with them.
PRESET_FLAGS = re.compile(r"RUSTFLAGS|CARGO_ENCODED_RUSTFLAGS|CARGO_BUILD_RUSTFLAGS|CARGO_TARGET_\w+_RUSTFLAGS")
CONFIG_RUSTFLAGS = re.compile(r"(?m)^[^#\n]*\brustflags\s*=")


class BuildError(Exception):
    """The release build cannot run with this environment."""


def remap_rules(home, cargo_home, rustup_home, root, target_dir):
    """General to specific, because rustc applies the last matching rule."""
    return [(home, "~"), (cargo_home, "/cargo"), (rustup_home, "/rustup"),
            (root, "/token-ashtray"), (target_dir, "/target")]


def rustflags(rules, target):
    flags = []
    for source, token in rules:
        forms = [str(source)]
        if re.match(r"[A-Za-z]:", forms[0]):
            # rustc can see either drive-letter case for the same directory.
            forms.append(forms[0][0].swapcase() + forms[0][1:])
        flags += [f"--remap-path-prefix={form}={token}" for form in forms]
    if target.endswith("-windows-msvc"):
        flags += ["-C", "target-feature=+crt-static"]
    return flags


def build_environment(base, flags):
    """0x1F-separated, so a path containing spaces stays one argument."""
    return {**base, "CARGO_ENCODED_RUSTFLAGS": "\x1f".join(flags)}


def refuse_preset_flags(env, config_files):
    preset = sorted(name for name in env if PRESET_FLAGS.fullmatch(name.upper()))
    if preset:
        raise BuildError(f"unset {', '.join(preset)}: release builds set every compiler flag themselves")
    for path in config_files:
        try:
            text = Path(path).read_text(encoding="utf-8")
        except FileNotFoundError:
            continue
        if CONFIG_RUSTFLAGS.search(text):
            raise BuildError(f"remove rustflags from {shown(path)}: release builds set every compiler flag themselves")


def shown(path):
    """A path for console output, with the home directory (and account name) as ~."""
    try:
        return str(Path("~") / Path(path).relative_to(Path.home()))
    except ValueError:
        return str(path)


def cargo_config_files(cargo_home):
    directories = [*(directory / ".cargo" for directory in (ROOT, *ROOT.parents)), cargo_home]
    return [directory / name for directory in directories for name in ("config.toml", "config")]


def capture(command):
    return subprocess.run(command, cwd=ROOT, check=True, capture_output=True, text=True).stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--target", help="target triple; required for x86_64-pc-windows-msvc")
    parser.add_argument("--bin", action="append", required=True, dest="bins", help="binary to build (repeatable)")
    args = parser.parse_args()
    home = Path.home()
    cargo_home = Path(os.environ.get("CARGO_HOME") or home / ".cargo")
    rustup_home = Path(os.environ.get("RUSTUP_HOME") or home / ".rustup")
    try:
        refuse_preset_flags(os.environ, cargo_config_files(cargo_home))
        host = re.search(r"(?m)^host: (\S+)$", capture(["rustc", "-vV"])).group(1)
        target = args.target or host
        if target.endswith("-windows-msvc") and not args.target:
            raise BuildError(f"pass --target {target} so the static runtime is not applied to build scripts")
        target_dir = Path(json.loads(capture(["cargo", "metadata", "--locked", "--no-deps",
                                              "--format-version", "1"]))["target_directory"])
        flags = rustflags(remap_rules(home, cargo_home, rustup_home, ROOT, target_dir), target)
        command = ["cargo", "build", "--release", "--locked"]
        command += ["--target", args.target] if args.target else []
        for name in args.bins:
            command += ["--bin", name]
        subprocess.run(command, cwd=ROOT, env=build_environment(os.environ, flags), check=True)
        output = target_dir / (args.target or "") / "release"
        suffix = ".exe" if "-windows-" in target else ""
        executables = [output / (name + suffix) for name in args.bins]
        require_clean({path.name: path.read_bytes() for path in executables}, extra_local=[target_dir])
    except (BuildError, LeakError) as error:
        sys.exit(f"release_build: {error}")
    except subprocess.CalledProcessError as error:
        sys.exit(error.returncode)
    for path in executables:
        print(path)


if __name__ == "__main__":
    main()
