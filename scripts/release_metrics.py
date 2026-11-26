"""Record measured binary size and RSS; unknown measurements stay null."""
import argparse, ctypes, hashlib, json, os, platform, subprocess, time
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('binary', type=Path)
p.add_argument('--output', type=Path, required=True)
p.add_argument('--pid', type=int)
p.add_argument('--seconds', type=float, default=5)
a = p.parse_args()
process = None
if a.pid is None and os.environ.get("CI") != "true":
    p.error("Use --pid for an existing desktop instance; automatic process sampling is CI-only")
if a.pid is None:
    # CI owns this credential-free process. Preserve native startup diagnostics
    # in the job log so a rejected package has an actionable failure reason.
    process = subprocess.Popen([str(a.binary.resolve()), '--autostart'], stdout=subprocess.DEVNULL)
    a.pid = process.pid
    time.sleep(a.seconds)
rss = None
reason = None
startup_failed = False
startup_exit_code = process.poll() if process else None
try:
    if platform.system() == 'Linux':
        status = Path(f'/proc/{a.pid}/status').read_text()
        rss = int(next(x for x in status.splitlines() if x.startswith('VmRSS:')).split()[1]) * 1024
    elif platform.system() == 'Windows':
        from ctypes import wintypes
        class Counters(ctypes.Structure):
            _fields_ = [('cb', wintypes.DWORD), ('PageFaultCount', wintypes.DWORD)] + [(name, ctypes.c_size_t) for name in ('PeakWorkingSetSize', 'WorkingSetSize', 'QuotaPeakPagedPoolUsage', 'QuotaPagedPoolUsage', 'QuotaPeakNonPagedPoolUsage', 'QuotaNonPagedPoolUsage', 'PagefileUsage', 'PeakPagefileUsage')]
        kernel = ctypes.windll.kernel32
        kernel.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel.OpenProcess.restype = wintypes.HANDLE
        kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        ctypes.windll.psapi.GetProcessMemoryInfo.argtypes = [wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD]
        handle = kernel.OpenProcess(0x0400 | 0x0010, False, a.pid)
        if not handle:
            raise OSError('Could not open measurement process')
        counters = Counters(); counters.cb = ctypes.sizeof(counters)
        if not ctypes.windll.psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
            raise OSError('Working set measurement unavailable')
        rss = counters.WorkingSetSize
        kernel.CloseHandle(handle)
    else:
        reason = 'Unsupported measurement platform'
except (OSError, StopIteration, ValueError) as error:
    reason = str(error)
finally:
    if process and process.poll() is not None:
        startup_exit_code = process.returncode
        startup_failed = True
        reason = f"Application exited during startup with status {startup_exit_code}"
    elif process and rss is None:
        startup_failed = True
        reason = reason or "Could not verify startup memory of CI-owned process"
    # The CI-created process has no user CLI credentials. --pid is read-only and
    # never stops a caller-owned desktop instance.
    if process:
        process.terminate()
        try: process.wait(timeout=25)
        except subprocess.TimeoutExpired: process.kill(); process.wait()
report = {'platform': platform.platform(), 'binary_bytes': a.binary.stat().st_size,
          'binary_sha256': hashlib.sha256(a.binary.read_bytes()).hexdigest(),
          'idle_rss_bytes': rss, 'measurement_note': reason,
          'startup_mode': 'autostart requested; validate tray visibility on target desktop',
          'startup_exit_code': startup_exit_code,
          'targets': {'binary_bytes': 20_000_000, 'idle_rss_bytes': 15_000_000}}
a.output.parent.mkdir(parents=True, exist_ok=True)
a.output.write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report))

if startup_failed:
    raise SystemExit(1)
