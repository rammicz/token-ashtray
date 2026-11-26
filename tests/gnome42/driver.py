import json
import subprocess
import time
from pathlib import Path
import gi
gi.require_version("Gdk", "3.0")
from gi.repository import Gio, GLib, Gdk

bus = Gio.bus_get_sync(Gio.BusType.SESSION,None)
def evaluate(js):
    ok, value = bus.call_sync('org.gnome.Shell','/org/gnome/Shell','org.gnome.Shell','Eval',GLib.Variant('(s)',(js,)),None,Gio.DBusCallFlags.NONE,5000,None).unpack()
    assert ok, value
    return json.loads(value)

def capture(name):
    root = Gdk.get_default_root_window()
    image = Gdk.pixbuf_get_from_window(root, 0, 0, root.get_width(), root.get_height())
    image.savev('/work/'+name+'.png', 'png', [], [])

def click(x,y,button=1):
    with open('/work/pointer.jsonl','a') as log:
        log.write(json.dumps({'time':time.monotonic(),'x':x,'y':y,'button':button})+'\n')
    subprocess.run(['xdotool','mousemove',str(x),str(y),'click',str(button)],check=True)
    time.sleep(.8)

def entries(item='token-ashtray'):
    path = Path('/work/'+item+'.jsonl')
    return [json.loads(s) for s in path.read_text().splitlines()]

def count(item='token-ashtray'):
    return sum(e['method']=='Activate' for e in entries(item))

Path('/work/mountinfo.txt').write_text(Path('/proc/self/mountinfo').read_text())
Path('/work/network-route.txt').write_text(Path('/proc/net/route').read_text())
Path('/work/processes.txt').write_text(subprocess.check_output(['ps','-eo','pid,ppid,args'],text=True))
subprocess.Popen(['python3','/harness/fixture.py','token-ashtray'])
subprocess.Popen(['python3','/harness/fixture.py','unrelated-item'])
time.sleep(6)
actors = evaluate("Object.values(imports.ui.main.panel.statusArea).filter(a=>a._indicator).map(a=>({id:a._indicator.id,pos:a.get_transformed_position(),size:a.get_transformed_size()}))")
Path('/work/actors.json').write_text(json.dumps(actors))
def geometry(item):
    a = next(a for a in actors if a.get('id')==item)
    return (round(a['pos'][0]+a['size'][0]/2),round(a['pos'][1]+a['size'][1]/2))
def menu_open(item):
    return evaluate("Object.values(imports.ui.main.panel.statusArea).find(a=>a._indicator?.id==='"+item+"').menu.isOpen")
x,y = geometry('token-ashtray')
capture('before')
click(x,y)
capture('single-left')
Path('/work/single-left.json').write_text(json.dumps({'activate':count(),'menu':menu_open('token-ashtray')}))
assert count()==1, f'physical single-left: expected 1 Activate, got {count()}; menu={menu_open("token-ashtray")}'
assert not menu_open('token-ashtray'), 'left opened menu'
print('SINGLE_LEFT_PASS',flush=True)
click(x,y)
assert count()==2 and not menu_open('token-ashtray'), 'spaced left click'
with open('/work/pointer.jsonl','a') as log:
    log.write(json.dumps({'time':time.monotonic(),'x':x,'y':y,'button':1,'repeat':2,'delay_ms':80})+'\n')
subprocess.run(['xdotool','mousemove',str(x),str(y),'click','--repeat','2','--delay','80','1'],check=True)
time.sleep(1)
assert count()==3 and not menu_open('token-ashtray'), 'double-click duplicated activation or opened menu'
subprocess.run(['xdotool','mousemove',str(x),str(y),'click','--repeat','3','--delay','80','1'],check=True)
time.sleep(1)
assert count()==4 and not menu_open('token-ashtray'), 'triple-click duplicated activation or opened menu'

for action in range(1,6):
    click(x,y,3)
    assert menu_open('token-ashtray') and count()==4, 'right-click activated or lost menu'
    items = evaluate("Object.values(imports.ui.main.panel.statusArea).find(a=>a._indicator?.id==='token-ashtray').menu._getMenuItems().map(a=>({text:a.label?.text,pos:a.get_transformed_position(),size:a.get_transformed_size()}))")
    Path('/work/menu-items.json').write_text(json.dumps(items))
    capture('menu-'+str(action))
    item = items[action-1]
    click(round(item['pos'][0]+item['size'][0]/2),round(item['pos'][1]+item['size'][1]/2))
    assert any(e['method']=='Event' and e['args'][0]==action and e['args'][1]=='clicked' for e in entries()), f'action {action} not selected'
    assert count()==4, 'menu action dispatched Activate'
# Keyboard still opens the native menu (upstream PanelMenu key handling).
evaluate("Object.values(imports.ui.main.panel.statusArea).find(a=>a._indicator?.id==='token-ashtray').grab_key_focus(); true")
subprocess.run(['xdotool','key','space'],check=True)
time.sleep(1)
assert menu_open('token-ashtray') and count()==4, 'keyboard menu changed'
subprocess.run(['xdotool','key','Escape'],check=True)
time.sleep(.8)
x2,y2 = geometry('unrelated-item')
click(x2,y2)
capture('unrelated-left')
assert count('unrelated-item')==0 and menu_open('unrelated-item'), 'unrelated single-left behavior changed'
subprocess.run(['xdotool','key','Escape'],check=True)
time.sleep(.8)
Path('/work/final-processes.txt').write_text(subprocess.check_output(['ps','-eo','pid,ppid,args'],text=True))
Path('/work/listeners.txt').write_text(subprocess.check_output(['ss','-ltnp'],text=True))
Path('/work/result.json').write_text(json.dumps({'single_left':True,'spaced_left':True,'double_click_activate_count':1,'triple_click_activate_count':1,'right_actions':5,'keyboard_menu':True,'unrelated_single_left':True}))
print('ALL_HOST_ROUTE_CHECKS_PASS',flush=True)
