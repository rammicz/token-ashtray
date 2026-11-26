"""Physical Nautilus/GNOME/eframe acceptance on the exact shipped executables."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time
import gi
gi.require_version("Gdk", "3.0")
from gi.repository import Gdk, Gio, GLib
import pyatspi
from menu_result import exact_visible_result

work = Path('/work')
home = Path.home()
download = home/'Downloads/token-ashtray-setup-linux-x86_64'
dest = home/'.local/opt/token-ashtray'
data = Path(os.environ['XDG_DATA_HOME'])
config = Path(os.environ['XDG_CONFIG_HOME'])
menu = data/'applications/token-ashtray.desktop'
setup_menu = data/'applications/token-ashtray-setup.desktop'
startup = config/'autostart/token-ashtray.desktop'
results = []

def command(*args):
    return subprocess.check_output(args, text=True).strip()

def keys(*args):
    subprocess.run(['xdotool','key',*args],check=True)
    time.sleep(.7)

def type_text(text):
    subprocess.run(['xdotool','type','--clearmodifiers',text],check=True)
    time.sleep(.7)

def capture(name):
    root = Gdk.get_default_root_window()
    Gdk.pixbuf_get_from_window(root,0,0,root.get_width(),root.get_height()).savev(str(work/(name+'.png')),'png',[],[])
    (work/(name+'-windows.txt')).write_text(command('xwininfo','-root','-tree'))

def window(title):
    for _ in range(40):
        run = subprocess.run(['xdotool','search','--onlyvisible','--name',title],capture_output=True,text=True)
        if run.returncode == 0:
            return run.stdout.splitlines()[-1]
        time.sleep(.3)
    capture('missing-window')
    raise AssertionError('No visible window '+title)

def activate(title):
    wid = window(title)
    subprocess.run(['xdotool','windowactivate','--sync',wid],check=True)
    time.sleep(.5)
    return wid

def open_nautilus(path):
    # Opening a directory is setup only; execution below is via Nautilus input.
    subprocess.Popen(['nautilus',str(path)],stdout=(work/'nautilus.log').open('a'),stderr=subprocess.STDOUT)
    time.sleep(2)
    activate('Downloads')

def open_download(name):
    open_nautilus(download.parent)
    # Ctrl+L with complete path selects the file; Return opens that selection.
    keys('ctrl+l');type_text(str(download));keys('Return');keys('Return')
    time.sleep(2)
    capture(name+'-opened')
    return activate('^Token Ashtray Setup$')

def setup_action(name,removing=False):
    wid = activate('^Token Ashtray Setup$')
    capture(name+'-before')
    # eframe's first keyboard-focusable widget is the install/remove button.
    keys('Tab');keys('Return')
    time.sleep(2)
    capture(name+'-after')
    if removing:
        assert not menu.exists() and not setup_menu.exists() and not startup.exists(),name
        assert not (dest/'token-ashtray').exists(),name
    else:
        assert menu.exists() and setup_menu.exists(),name
        inspection = json.loads(command(str(dest/'token-ashtray-setup'),'--inspect'))
        item = next(x for x in inspection['members'] if x['name']=='token-ashtray')
        assert hashlib.sha256((dest/'token-ashtray').read_bytes()).hexdigest()==item['sha256']
    # Close actual success window with normal window-manager close.
    keys('alt+F4')
    results.append(name)

def select_exact_search_result(name, target):
    # Result rank changes across GNOME sessions. Identify the visible native
    # label inside this private Shell, then act on that result only.
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    bus.call_sync(
        'org.a11y.Bus', '/org/a11y/bus', 'org.freedesktop.DBus.Properties', 'Set',
        GLib.Variant('(ssv)', ('org.a11y.Status', 'IsEnabled', GLib.Variant('b', True))),
        None, Gio.DBusCallFlags.NONE, 5000, None,
    )
    shell_pid = int((work/'gnome.pid').read_text())
    screen = Gdk.get_default_root_window()
    width, height = screen.get_width(), screen.get_height()
    errors = (AttributeError, NotImplementedError, LookupError, RuntimeError, GLib.Error)
    deadline = time.monotonic() + 25
    last = []
    while time.monotonic() < deadline:
        context = GLib.MainContext.default()
        for _ in range(100):
            if not context.pending():
                break
            context.iteration(False)
        apps = []
        for app in pyatspi.Registry.getDesktop(0):
            try:
                if app.get_process_id() == shell_pid:
                    apps.append(app)
            except errors:
                pass
        assert len(apps) <= 1, 'ambiguous private GNOME accessibility application'
        stack = [(app, 0) for app in apps]
        count = 0
        last = []
        while stack:
            node, depth = stack.pop()
            count += 1
            assert count <= 4000 and depth <= 40, 'native accessibility tree bound'
            try:
                node.clearCache()
                if node.name == target:
                    rect = node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
                    last.append({
                        'name': node.name, 'role': node.getRoleName(),
                        'showing': node.getState().contains(pyatspi.STATE_SHOWING),
                        'bounds': [rect.x, rect.y, rect.width, rect.height],
                    })
                for child in reversed(list(node)):
                    stack.append((child, depth + 1))
            except errors:
                pass
        info = exact_visible_result(last, target, width, height)
        if info is not None:
            x, y, w, h = info['bounds']
            center = (x + w//2, y + h//2)
            (work/(name+'-menu-selection.json')).write_text(json.dumps({
                'target': target, 'selected_exact_native_label': info, 'shell_pid': shell_pid,
                'click_point': list(center), 'result_rank_assumed': False,
                'method': 'normal xdotool click at exact visible native label bounds',
            }, indent=2))
            subprocess.run(['xdotool', 'mousemove', '--sync', str(center[0]), str(center[1]),
                            'click', '1'], check=True)
            return
        time.sleep(.2)
    (work/(name+'-menu-selection-failure.json')).write_text(json.dumps({
        'target': target, 'shell_pid': shell_pid, 'matching_native_labels': last,
    }, indent=2))
    capture(name+'-menu-selection-failure')
    raise AssertionError('Exact visible native GNOME result label unavailable')


def menu_launch(name, setup=False):
    target = 'Token Ashtray Setup' if setup else 'Token Ashtray'
    keys('Super_L')
    type_text(target)
    time.sleep(3)
    capture(name+'-menu-icon')
    select_exact_search_result(name, target)
    time.sleep(4)
    wid = activate('^'+target+'$')
    pid = command('xdotool', 'getwindowpid', wid)
    actual = Path('/proc')/pid/'exe'
    expected = dest/('token-ashtray-setup' if setup else 'token-ashtray')
    assert os.path.samefile(actual, expected), str(actual.resolve())
    argv = (Path('/proc')/pid/'cmdline').read_bytes().decode().rstrip('\0').split('\0')
    if setup:
        assert '--uninstall' in argv, 'wrong native setup action'
    else:
        assert '--ui-child' not in argv, 'unexpected detached child in tray-absent profile'
    (work/(name+'-app.json')).write_text(json.dumps({
        'pid': int(pid), 'exe': str(actual.resolve()),
        'sha256': hashlib.sha256(actual.read_bytes()).hexdigest(), 'argv': argv,
        'desktop_entry': str(setup_menu if setup else menu),
    }))
    if not setup:
        capture(name+'-application')
        # With no tray host, this is the root board; WM_CLOSE exits it normally.
        keys('alt+F4')
        time.sleep(2)

(work/'os-release.txt').write_text(Path('/etc/os-release').read_text())
(work/'mountinfo.txt').write_text(Path('/proc/self/mountinfo').read_text())
(work/'network-route.txt').write_text(Path('/proc/net/route').read_text())
(work/'processes-before.txt').write_text(command('ps','-eo','pid,ppid,args'))
keys('Escape')
if (work/'resume-profile').exists():
    # A new GNOME/DBus session with the installed profile is the desktop
    # re-login documented for desktops that cache their application list.
    menu_launch('relogin')
else:
    open_nautilus(download.parent)
    keys('ctrl+l');type_text(str(download));keys('Escape')
    keys('ctrl+a');keys('alt+Return');time.sleep(2);capture('download-properties')
    keys('ctrl+Page_Down');time.sleep(1);capture('download-permissions')
    root = pyatspi.Registry.getDesktop(0)
    def find_accessible(predicate):
        def visit(node):
            try:
                if predicate(node): return node
                for child in node:
                    found=visit(child)
                    if found:return found
            except (LookupError,RuntimeError):pass
        return visit(root)
    checkbox = find_accessible(lambda n:n.getRoleName()=='check box' and 'execut' in n.name.lower())
    assert checkbox is not None,'Nautilus executable permission checkbox absent'
    checkbox.queryAction().doAction(0)
    time.sleep(1);capture('download-executable-enabled')
    assert download.stat().st_mode & 0o100
    keys('alt+F4')
    open_download('fresh');setup_action('fresh-install')
    if (work/'install-only').exists():
        (work/'result.json').write_text(json.dumps({'installed_first_session':True,'passed':results,'desktop_relogin_pending':True}))
        print('NATIVE_DOWNLOADED_GUI_INSTALL_PASS',flush=True)
        raise SystemExit(0)
    menu_launch('fresh')
menu_launch('fresh-uninstall',setup=True);setup_action('fresh-uninstall',True)

# Offline legacy ZIP fixture: real earlier executable and exact #28 helper.
legacy=work/'legacy ZIP';legacy.mkdir()
for source,name in [('legacy-app','token-ashtray'),('install-linux.py','install-linux.py'),('token-ashtray-256.png','token-ashtray-256.png')]:
    (legacy/name).write_bytes((work/source).read_bytes())
(legacy/'token-ashtray').chmod(0o755)
subprocess.run(['python3',str(legacy/'install-linux.py'),'install'],check=True)
# Keep state byte-for-byte, including unknown TOML keys and credential canaries.
preferences=config/'token-ashtray/config.toml';preferences.parent.mkdir(parents=True,exist_ok=True)
preferences.write_text("unknown_setup_canary = 'retained'\n")
canaries=[home/'.claude/.credentials.json',home/'.codex/auth.json',home/'.cursor/auth.json',config/'gh/hosts.yml',home/'.grok/auth.json',home/'.kimi/credentials.json',home/'.gemini/oauth_creds.json']
for path in canaries:path.parent.mkdir(parents=True,exist_ok=True);path.write_text('offline canary '+str(path))
state=[preferences]+canaries
before={str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in state}
(work/'state-before.json').write_text(json.dumps(before,indent=2))
startup.parent.mkdir(parents=True,exist_ok=True)
exec_line=next(line for line in menu.read_text().splitlines() if line.startswith('Exec='))+' --autostart'
for disabled in (False,True):
    startup_body='[Desktop Entry]\nType=Application\n'+exec_line+'\nX-Token-Ashtray=true\nHidden='+str(disabled).lower()+'\nX-GNOME-Autostart-enabled='+str(not disabled).lower()+'\nX-Canary=preserved\n'
    startup.write_text(startup_body)
    if disabled:
        # Previous graphical install uses a separate, earlier real app payload.
        previous=home/'Downloads/previous-setup';previous.write_bytes((work/'previous-setup').read_bytes());previous.chmod(0o755)
        keys('ctrl+l');type_text(str(previous));keys('Return');keys('Return');time.sleep(2)
        setup_action('previous-gui-install')
    prefix='gui-disabled' if disabled else 'zip-enabled'
    old_hash=hashlib.sha256((dest/'token-ashtray').read_bytes()).hexdigest()
    for repeat in range(2):
        open_download(prefix+str(repeat));setup_action(prefix+'-upgrade-'+str(repeat))
        assert startup.read_text()==startup_body
        assert {str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in state}==before
    assert hashlib.sha256((dest/'token-ashtray').read_bytes()).hexdigest()!=old_hash
    menu_launch(prefix+'-remove',True);setup_action(prefix+'-remove',True)
    assert {str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in state}==before
    if not disabled:
        # Empty GUI-free baseline for next prior graphical installation.
        open_nautilus(download.parent)
(work/'state-after.json').write_text(json.dumps({str(p):hashlib.sha256(p.read_bytes()).hexdigest() for p in state},indent=2))
(work/'processes-after.txt').write_text(command('ps','-eo','pid,ppid,args'))
(work/'listeners.txt').write_text(command('ss','-ltnp'))
(work/'result.json').write_text(json.dumps({'passed':results,'preserved':before,'network':'private unshared network namespace','desktop':'Ubuntu 22.04 GNOME 42 X11'},indent=2))
print('ALL_NATIVE_SETUP_CHECKS_PASS',flush=True)
