"""Run the real setup in a private Ubuntu GNOME desktop; never bind user profiles."""
import argparse
import hashlib
import json
import shutil
from pathlib import Path
import subprocess

p = argparse.ArgumentParser(description=__doc__)
p.add_argument("work", type=Path)
p.add_argument("artifact", type=Path)
p.add_argument("--legacy-app", type=Path, required=True)
p.add_argument("--previous-setup", type=Path, required=True)
p.add_argument("--absent-applications", action="store_true")
p.add_argument("--install-only", action="store_true")
p.add_argument("--resume-profile", type=Path)
a = p.parse_args()
a.work.mkdir()
if a.install_only:
    (a.work / "install-only").touch()
if a.absent_applications:
    (a.work / "absent-applications").touch()
if a.resume_profile:
    previous_inputs = json.loads((a.resume_profile / "inputs.json").read_text())
    assert previous_inputs["download"]["sha256"] == hashlib.sha256(a.artifact.read_bytes()).hexdigest()
    for name in ("home profile", "data path", "config path", "cache path"):
        shutil.copytree(a.resume_profile / name, a.work / name)
    (a.work / "resume-profile").write_text(str(a.resume_profile.resolve()))
harness = Path(__file__).resolve().parent
root = harness.parents[1]
inputs = {"download": a.artifact, "legacy-app": a.legacy_app, "previous-setup": a.previous_setup,
          "install-linux.py": root / "install-linux.py", "token-ashtray-256.png": root / "assets/application/token-ashtray-256.png"}
for name, source in inputs.items():
    (a.work / name).write_bytes(source.read_bytes())
(a.work / "inputs.json").write_text(json.dumps({name: {"source": str(source.resolve()), "sha256": hashlib.sha256(source.read_bytes()).hexdigest()} for name, source in inputs.items()}, indent=2))
(a.work / "harness-sha256.json").write_text(json.dumps({x.name: hashlib.sha256(x.read_bytes()).hexdigest() for x in harness.iterdir() if x.is_file()}, indent=2))
cmd = ["bwrap", "--unshare-all", "--die-with-parent", "--new-session"]
for path in ["/usr", "/lib", "/lib64", "/etc"]:
    cmd += ["--ro-bind", path, path]
cmd += ["--symlink", "usr/bin", "/bin", "--symlink", "usr/sbin", "/sbin", "--proc", "/proc", "--dev", "/dev",
        "--tmpfs", "/tmp", "--tmpfs", "/run", "--dir", "/home", "--dir", "/root", "--bind", str(a.work.resolve()), "/work",
        "--ro-bind", str(harness), "/harness", "--chdir", "/work", "--clearenv", "--setenv", "PATH", "/usr/bin:/bin",
        "--setenv", "LANG", "C.UTF-8", "--", "dbus-run-session", "--", "bash", "/harness/session.sh"]
with (a.work / "launch.log").open("w") as log:
    process = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    (a.work / "outer.pid").write_text(str(process.pid))
    try:
        code = process.wait(timeout=480)
    except subprocess.TimeoutExpired:
        process.terminate()
        process.wait(timeout=10)
        raise
    finally:
        (a.work / "cleanup.json").write_text(json.dumps({"outer_pid": process.pid, "reaped": process.poll() is not None,
                                                        "private_pid_namespace_destroyed": process.poll() is not None}))
(a.work / "exit.json").write_text(json.dumps({"returncode": code}))
raise SystemExit(code)
