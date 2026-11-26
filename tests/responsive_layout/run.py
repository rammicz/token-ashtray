"""Capture the actual UI child ELF with offline normalized quota snapshots."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('work', type=Path)
p.add_argument('--isolated-container', action='store_true', help='Run only inside an externally isolated, network-disabled disposable Docker container')
p.add_argument('binary', type=Path)
p.add_argument('--expect-responsive', action='store_true')
p.add_argument('--fixture', type=Path)
p.add_argument('--themes', nargs='+', choices=['dark','light'], default=['dark'])
p.add_argument('--mode', choices=['used','remaining'], default='remaining')
p.add_argument('--freeze-clock', action='store_true')
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
    a.work.mkdir()
harness = Path(__file__).resolve().parent
fixture = a.fixture or harness.parent/'fixtures/responsive-layout.json'
(a.work/'options.json').write_text(json.dumps({'themes':a.themes,'mode':a.mode,'freeze_clock':a.freeze_clock}))
if a.freeze_clock:
    subprocess.run(['cc','-shared','-fPIC','-O2',str(harness/'frozen-clock.c'),'-o',str(a.work/'frozen-clock.so')],check=True)
    frozen=subprocess.check_output(['date','+%s'],env={'PATH':'/usr/bin:/bin','LD_PRELOAD':str((a.work/'frozen-clock.so').resolve())},text=True).strip()
    assert frozen=='1800000000',frozen
    (a.work/'clock-proof.json').write_text(json.dumps({'unix_seconds':int(frozen),'monotonic_clock_unmodified':True,'interposer_sha256':hashlib.sha256((a.work/'frozen-clock.so').read_bytes()).hexdigest()}))
(a.work/'token-ashtray').write_bytes(a.binary.read_bytes())
(a.work/'token-ashtray').chmod(0o755)
(a.work/'snapshot.json').write_bytes(fixture.read_bytes())
if a.expect_responsive:
    (a.work/'expect-responsive').touch()
(a.work/'inputs.json').write_text(json.dumps({'binary':str(a.binary.resolve()), 'binary_sha256':hashlib.sha256(a.binary.read_bytes()).hexdigest(), 'fixture_sha256':hashlib.sha256(fixture.read_bytes()).hexdigest(), 'harness':{x.name:hashlib.sha256(x.read_bytes()).hexdigest() for x in harness.iterdir() if x.is_file()}},indent=2))
cmd=['bwrap','--unshare-all','--die-with-parent','--new-session']
for path in ['/usr','/lib','/lib64','/etc']:
    cmd+=['--ro-bind',path,path]
cmd+=['--symlink','usr/bin','/bin','--symlink','usr/sbin','/sbin','--proc','/proc','--dev','/dev','--tmpfs','/tmp','--tmpfs','/run','--dir','/home','--dir','/root','--bind',str(a.work.resolve()),'/work','--ro-bind',str(harness),'/harness','--clearenv','--chdir','/work','--setenv','PATH','/usr/bin:/bin','--setenv','LANG','C.UTF-8','--','dbus-run-session','--','bash','/harness/session.sh']
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
with (a.work/'launch.log').open('w') as log:
    process=subprocess.Popen(cmd,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
    (a.work/'outer.pid').write_text(str(process.pid))
    try:
        code=process.wait(timeout=480)
    except subprocess.TimeoutExpired:
        process.terminate();process.wait(timeout=10)
        raise
    finally:
        (a.work/'cleanup.json').write_text(json.dumps({'outer_pid':process.pid,'reaped':process.poll() is not None,'private_pid_namespace_destroyed':not a.isolated_container and process.poll() is not None, 'outer_container_cleanup_required':a.isolated_container}))
(a.work/'exit.json').write_text(json.dumps({'returncode':code}))
raise SystemExit(code)
