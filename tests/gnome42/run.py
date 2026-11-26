"""Launch a fresh, network-isolated GNOME session. Work directory must be new."""
import argparse
import json
import hashlib
import subprocess
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('work', type=Path)
p.add_argument('--isolated-container', action='store_true', help='Run only inside an externally isolated, network-disabled disposable Docker container')
p.add_argument('--route', type=Path)
a = p.parse_args()
if a.isolated_container:
    if not Path('/.dockerenv').is_file() or a.work.resolve() != Path('/work'):
        p.error('--isolated-container requires Docker and a fresh /work directory')
    routes = Path('/proc/net/route').read_text().splitlines()[1:]
    if any(line.split()[1] == '00000000' for line in routes if line.split()):
        p.error('--isolated-container requires Docker --network none')
    if a.work.exists() and any(a.work.iterdir()):
        p.error('/work must be empty')
    a.work.mkdir(exist_ok=True)
else:
    a.work.mkdir()  # Refuse accidental reuse of profiles/evidence.
if a.route:
    (a.work/'route.zip').write_bytes(a.route.read_bytes())
harness = Path(__file__).resolve().parent
(a.work/'harness-sha256.json').write_text(json.dumps({p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(harness.iterdir()) if p.is_file()},indent=2))
if a.route:
    (a.work/'route.sha256').write_text(hashlib.sha256(a.route.read_bytes()).hexdigest())
cmd = ['bwrap','--unshare-all','--die-with-parent','--new-session']
for path in ['/usr','/lib','/lib64','/etc']:
    cmd += ['--ro-bind',path,path]
cmd += ['--symlink','usr/bin','/bin','--symlink','usr/sbin','/sbin','--proc','/proc','--dev','/dev',
        '--tmpfs','/tmp','--tmpfs','/run','--dir','/home','--dir','/root','--bind',str(a.work.resolve()),'/work',
        '--ro-bind',str(harness),'/harness','--chdir','/work','--clearenv','--setenv','PATH','/usr/bin:/bin',
        '--setenv','LANG','C.UTF-8','--','dbus-run-session','--','bash','/harness/session.sh']
if a.isolated_container:
    # The outer container must have no host display, session bus, home, or
    # credential mounts. Its termination reaps the private native session.
    target = Path('/harness')
    if target.exists() and target.resolve() != harness:
        p.error('/harness already refers to a different harness')
    if not target.exists():
        target.symlink_to(harness, target_is_directory=True)
    cmd = ['env', '-i', 'PATH=/usr/bin:/bin', 'LANG=C.UTF-8',
           'dbus-run-session', '--', 'bash', '/harness/session.sh']
with (a.work/'launch.log').open('w') as f:
    run = subprocess.Popen(cmd, stdout=f, stderr=subprocess.STDOUT, start_new_session=True)
    (a.work/'outer.pid').write_text(str(run.pid))
    try:
        code = run.wait(timeout=150)
    except subprocess.TimeoutExpired:
        run.terminate()
        run.wait(timeout=10)
        raise
    finally:
        (a.work/'cleanup.json').write_text(json.dumps({'outer_pid':run.pid, 'reaped':run.poll() is not None, 'private_pid_namespace_destroyed':not a.isolated_container and run.poll() is not None, 'outer_container_cleanup_required':a.isolated_container}))
(a.work/'exit.json').write_text(json.dumps({'returncode':code}))
raise SystemExit(code)
