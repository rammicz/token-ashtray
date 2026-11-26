"""Measure native card pixels/gauge accessibility and activate real UI controls."""
from collections import deque
import ctypes
import json
import os
from pathlib import Path
import subprocess
import time
import gi

gi.require_version('Gdk','3.0')
from gi.repository import Gdk, Gio, GLib
import pyatspi

work=Path('/work')
bus=Gio.bus_get_sync(Gio.BusType.SESSION,None)
a11y={}
for key in ['IsEnabled','ScreenReaderEnabled']:
    a11y[key]=bus.call_sync('org.a11y.Bus','/org/a11y/bus','org.freedesktop.DBus.Properties','Get',GLib.Variant('(ss)',('org.a11y.Status',key)),None,Gio.DBusCallFlags.NONE,5000,None).unpack()[0]
bus.call_sync('org.a11y.Bus','/org/a11y/bus','org.freedesktop.DBus.Properties','Set',GLib.Variant('(ssv)',('org.a11y.Status','IsEnabled',GLib.Variant('b',True))),None,Gio.DBusCallFlags.NONE,5000,None)
(work/'a11y-before.json').write_text(json.dumps(a11y,default=str))
# The URL receiver is test infrastructure: actual OpenUrl activation is retained,
# but its browser process logs the fixture URI instead of accessing the network.
receiver=work/'bin/record-url'
receiver.write_text("#!/usr/bin/python3\nimport json,sys\nwith open('/work/links.jsonl','a') as f:f.write(json.dumps(sys.argv[1:])+'\\n')\n")
receiver.chmod(0o755)
options=json.loads((work/'options.json').read_text())
cards=json.loads((work/'snapshot.json').read_text())
labels=[m['label'] for card in cards for m in card['meters']]


def command(*args):
    return subprocess.check_output(args,text=True).strip()


def geometry(wid):
    # Read live X geometry and translate (0,0), independent of toolkit caches
    # and xdotool's doubled reparenting origin on this GNOME baseline.
    xlib=ctypes.CDLL('libX11.so.6')
    xlib.XOpenDisplay.argtypes=[ctypes.c_char_p];xlib.XOpenDisplay.restype=ctypes.c_void_p
    xlib.XDefaultRootWindow.argtypes=[ctypes.c_void_p];xlib.XDefaultRootWindow.restype=ctypes.c_ulong
    xlib.XGetGeometry.argtypes=[ctypes.c_void_p,ctypes.c_ulong,ctypes.POINTER(ctypes.c_ulong),ctypes.POINTER(ctypes.c_int),ctypes.POINTER(ctypes.c_int),ctypes.POINTER(ctypes.c_uint),ctypes.POINTER(ctypes.c_uint),ctypes.POINTER(ctypes.c_uint),ctypes.POINTER(ctypes.c_uint)]
    xlib.XTranslateCoordinates.argtypes=[ctypes.c_void_p,ctypes.c_ulong,ctypes.c_ulong,ctypes.c_int,ctypes.c_int,ctypes.POINTER(ctypes.c_int),ctypes.POINTER(ctypes.c_int),ctypes.POINTER(ctypes.c_ulong)]
    xlib.XCloseDisplay.argtypes=[ctypes.c_void_p]
    display=xlib.XOpenDisplay(None);assert display
    try:
        root=ctypes.c_ulong();child=ctypes.c_ulong();x=ctypes.c_int();y=ctypes.c_int()
        w=ctypes.c_uint();h=ctypes.c_uint();border=ctypes.c_uint();depth=ctypes.c_uint()
        assert xlib.XGetGeometry(display,int(wid),ctypes.byref(root),ctypes.byref(x),ctypes.byref(y),ctypes.byref(w),ctypes.byref(h),ctypes.byref(border),ctypes.byref(depth))
        assert xlib.XTranslateCoordinates(display,int(wid),xlib.XDefaultRootWindow(display),0,0,ctypes.byref(x),ctypes.byref(y),ctypes.byref(child))
        return {'X':x.value,'Y':y.value,'WIDTH':w.value,'HEIGHT':h.value}
    finally:
        xlib.XCloseDisplay(display)


def window(title,pid):
    for _ in range(60):
        run=subprocess.run(['xdotool','search','--onlyvisible','--name',title],capture_output=True,text=True)
        if run.returncode==0:
            for wid in reversed(run.stdout.splitlines()):
                owner=subprocess.run(['xdotool','getwindowpid',wid],capture_output=True,text=True)
                if owner.returncode==0 and int(owner.stdout)==pid:return wid
        time.sleep(.2)
    (work/'missing-windows.txt').write_text(command('xwininfo','-root','-tree'))
    root=Gdk.get_default_root_window()
    Gdk.pixbuf_get_from_window(root,0,0,root.get_width(),root.get_height()).savev(str(work/'missing-desktop.png'),'png',[],[])
    raise AssertionError('missing native window '+title)


def walk(node):
    yield node
    try:
        for child in node:
            yield from walk(child)
    except (LookupError,RuntimeError):
        pass


def gauges(bounds):
    found=[]
    for node in walk(pyatspi.Registry.getDesktop(0)):
        try:
            if node.getRoleName() not in ('link','progress bar'):continue
            label=next((s for s in labels if node.name.startswith(s+':')),None)
            if label is None:continue
            r=node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
            if bounds['X']<=r.x<bounds['X']+bounds['WIDTH'] and bounds['Y']<=r.y<bounds['Y']+bounds['HEIGHT']:
                found.append({'label':label,'name':node.name,'rect':[r.x-bounds['X'],r.y-bounds['Y'],r.width,r.height],'node':node})
        except (LookupError,RuntimeError,NotImplementedError):
            pass
    return found


def card_pixels(pixbuf,theme):
    # Cards are connected components of the real renderer's dark surface color.
    pixels=pixbuf.get_pixels();stride=pixbuf.get_rowstride();channels=pixbuf.get_n_channels()
    width=pixbuf.get_width();height=pixbuf.get_height()
    selected=set()
    for y in range(height):
        row=y*stride
        for x in range(width):
            offset=row+x*channels
            if pixels[offset:offset+3]==(bytes([25,30,38]) if theme=='dark' else bytes([255,255,255])):selected.add(y*width+x)
    found=[]
    while selected:
        first=selected.pop();pending=deque([first]);xmin=xmax=first%width;ymin=ymax=first//width;area=0
        while pending:
            point=pending.popleft();x=point%width;y=point//width;area+=1
            xmin=min(xmin,x);xmax=max(xmax,x);ymin=min(ymin,y);ymax=max(ymax,y)
            for nxt in [point-width,point+width]+([point-1] if x else [])+([point+1] if x+1<width else []):
                if nxt in selected:selected.remove(nxt);pending.append(nxt)
        if xmax-xmin>100 and ymax-ymin>80 and area>5000:
            found.append({'left':xmin,'top':ymin,'right':xmax+1,'bottom':ymax+1,'width':xmax+1-xmin,'height':ymax+1-ymin,'surface_pixels':area})
    return sorted(found,key=lambda r:(r['top'],r['left']))


def link_count():
    return len((work/'links.jsonl').read_text().splitlines()) if (work/'links.jsonl').exists() else 0


results=[]
for theme,width in [(theme,width) for theme in options['themes'] for width in [1000,360]]:
    prefix=theme+'-' if options['themes']!=['dark'] else ''
    case=prefix+str(width)
    config=work/f'profile-{case}/config/token-ashtray';config.mkdir(parents=True)
    config.joinpath('config.toml').write_text(f'''[board]
order = {json.dumps([card['id'] for card in cards])}
theme_preference = "{theme}"
quota_display_mode = "{options['mode']}"
[[panels]]
id = 0
x = 20.0
y = 40.0
width = {width}.0
height = 1180.0
[[panels]]
id = 1
x = {width+80}.0
y = 40.0
width = {width}.0
height = 1180.0
''')
    env={**os.environ,'XDG_CONFIG_HOME':str(config.parent),'BROWSER':str(receiver),'PATH':'/work/bin:/usr/bin:/bin'}
    if options['freeze_clock']:env['LD_PRELOAD']='/work/frozen-clock.so'
    with (work/f'app-{case}.log').open('w') as log:
        process=subprocess.Popen(['/work/token-ashtray','--ui-child'],stdin=subprocess.PIPE,stdout=log,stderr=subprocess.STDOUT,text=True,env=env)
        def send(kind,data=None):
            message={'kind':kind}
            if data is not None:message['data']=data
            process.stdin.write(json.dumps(message)+'\n');process.stdin.flush()
        send('Snapshot',{c['id']:c for c in cards});send('Action','OpenBoard')
        ids=[('board',window('^Token Ashtray$',process.pid))]
        extra=subprocess.run(['xdotool','search','--onlyvisible','--all','--pid',str(process.pid),'--name','Panel'],capture_output=True,text=True)
        assert extra.returncode==1,'legacy detached entry opened a quota window'
        time.sleep(4)
        for _ in range(3):
            send('Action','OpenBoard')
            time.sleep(.2)
        boards=command('xdotool','search','--onlyvisible','--all','--pid',str(process.pid),'--name','^Token Ashtray$').splitlines()
        assert boards==[ids[0][1]],'repeated OpenBoard must use the same quota window'
        for name,wid in ids:
            subprocess.run(['xdotool','windowactivate','--sync',wid],check=True);time.sleep(.3)
            bounds=geometry(wid);assert bounds['WIDTH']==width,bounds
            subprocess.run(['xdotool','mousemove','2350','1500'],check=True);time.sleep(1)
            root=Gdk.get_default_root_window()
            image=Gdk.pixbuf_get_from_window(root,bounds['X'],bounds['Y'],bounds['WIDTH'],bounds['HEIGHT'])
            image.savev(str(work/f'{prefix}{name}-{width}.png'),'png',[],[])
            outer=card_pixels(image,theme)
            assert len(outer)==len(cards),outer
            visible=gauges(bounds)
            (work/f'{prefix}{name}-{width}-a11y-all.json').write_text(json.dumps([{'name':n.name,'role':n.getRoleName()} for n in walk(pyatspi.Registry.getDesktop(0)) if n.name],indent=2))
            assert len(visible)==len(labels),[(v['label'],v['rect']) for v in visible]
            report={'width':width,'native_title':command('xdotool','getwindowname',wid),'native_pid':int(command('xdotool','getwindowpid',wid)),'window':bounds,'cards':outer,'meters':[{k:v for k,v in m.items() if k!='node'} for m in visible]}
            first_top=outer[0]['top'];report['first_row_height']=max(r['bottom'] for r in outer if abs(r['top']-first_top)<=1)-first_top
            if (work/'expect-responsive').exists() and width==1000:
                assert abs(outer[0]['width']-outer[1]['width'])<=1 and outer[0]['top']==outer[1]['top']
                assert outer[2]['top']>=max(outer[0]['bottom'],outer[1]['bottom'])
                a,b=[next(m['rect'] for m in visible if m['label']==label) for label in labels[:2]]
                assert abs(a[1]-b[1])<=1 and abs(a[2]-b[2])<=1 and a[0]+a[2]<b[0]
            (work/f'{prefix}{name}-{width}.json').write_text(json.dumps(report,indent=2))
            target=next(m for m in visible if m['label']==labels[0]);x,y,w,h=target['rect']
            before=link_count()
            subprocess.run(['xdotool','mousemove',str(bounds['X']+x+w//2),str(bounds['Y']+y+h//2),'click','1'],check=True)
            time.sleep(1)
            assert link_count()==before+1,'native pointer usage activation'
            focused=None
            for _ in range(24):
                subprocess.run(['xdotool','key','Tab'],check=True);time.sleep(.25)
                focused=next((m for m in gauges(bounds) if m['node'].getState().contains(pyatspi.STATE_FOCUSED)),None)
                if focused:break
            assert focused is not None,'native Tab reaches a quota link'
            report['keyboard_focused_meter']=focused['label']
            root=Gdk.get_default_root_window()
            Gdk.pixbuf_get_from_window(root,bounds['X'],bounds['Y'],bounds['WIDTH'],bounds['HEIGHT']).savev(str(work/f'{prefix}{name}-{width}-focus.png'),'png',[],[])
            subprocess.run(['xdotool','key','Return'],check=True);time.sleep(1)
            assert link_count()==before+2,'native focused keyboard usage activation'
            report['native_pointer_and_keyboard_usage']=True
            (work/f'{prefix}{name}-{width}.json').write_text(json.dumps(report,indent=2));results.append(f'{prefix}{name}-{width}')
        if width==1000:
            subprocess.run(['xdotool','key','alt+F4'],check=True)
            for _ in range(50):
                messages=[]
                for line in (work/f'app-{case}.log').read_text().splitlines():
                    try:messages.append(json.loads(line))
                    except json.JSONDecodeError:pass
                if any(isinstance(message,dict) and message.get('kind')=='Closed' and message.get('data',{}).get('quit') is False for message in messages):break
                time.sleep(.1)
            else:raise AssertionError('board close did not report close-to-tray')
            send('CloseAcknowledged')
        else:
            send('CloseAcknowledged');send('Shutdown')
        process.wait(timeout=15)
        assert process.returncode==0,process.returncode
        (work/f'app-{case}-exit.json').write_text(json.dumps({'pid':process.pid,'exit':process.returncode}))
(work/'processes.txt').write_text(command('ps','-eo','pid,ppid,args'))
(work/'listeners.txt').write_text(command('ss','-ltnp'))
(work/'result.json').write_text(json.dumps({'passed':results,'offline':True,'real_ui_child_single_board':True},indent=2))
print('NATIVE_RESPONSIVE_FIXTURE_PASS',flush=True)
