"""Real SNI and DBusMenu fixture; calls are logged, never sent to the app."""
import json
import sys
import time
from gi.repository import Gio, GLib

item_id = sys.argv[1]
log = open('/work/' + item_id + '.jsonl', 'a', buffering=1)
bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
labels = ['Open board', 'Refresh', 'Autostart', 'About', 'Quit']

def record(method, args):
    log.write(json.dumps({'time':time.monotonic(),'method': method, 'args': args}) + '\n')

sni = '''<node><interface name="org.kde.StatusNotifierItem">
<method name="Activate"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
<method name="SecondaryActivate"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
<method name="ContextMenu"><arg type="i" direction="in"/><arg type="i" direction="in"/></method>
<method name="Scroll"><arg type="i" direction="in"/><arg type="s" direction="in"/></method>
''' + ''.join('<property name="%s" type="%s" access="read"/>' % p for p in [
('Id','s'),('Title','s'),('Category','s'),('Status','s'),('IconName','s'),('Menu','o'),('ItemIsMenu','b')]) + '</interface></node>'
props = {'Id': ('s', item_id), 'Title': ('s', item_id), 'Category': ('s','ApplicationStatus'),
         'Status': ('s','Active'), 'IconName': ('s','utilities-terminal'), 'Menu': ('o','/Menu'), 'ItemIsMenu': ('b',False)}

def method(conn, sender, path, interface, name, params, invocation):
    args = params.unpack()
    record(name, args)
    if name == 'GetLayout':
        children = [GLib.Variant('(ia{sv}av)', (i+1, {'label': GLib.Variant('s', label), 'enabled': GLib.Variant('b',True), 'visible': GLib.Variant('b',True)}, [])) for i,label in enumerate(labels)]
        invocation.return_value(GLib.Variant('(u(ia{sv}av))', (1,(0,{'children-display': GLib.Variant('s','submenu')},children))))
    elif name == 'GetGroupProperties':
        invocation.return_value(GLib.Variant('(a(ia{sv}))', ([(i+1,{'label': GLib.Variant('s',label), 'enabled':GLib.Variant('b',True), 'visible':GLib.Variant('b',True)}) for i,label in enumerate(labels)],)))
    elif name == 'AboutToShow':
        invocation.return_value(GLib.Variant('(b)',(False,)))
    elif name == 'EventGroup':
        invocation.return_value(GLib.Variant('(ai)',([],)))
    else:
        invocation.return_value(None)

bus.register_object('/StatusNotifierItem', Gio.DBusNodeInfo.new_for_xml(sni).interfaces[0], method,
                    lambda c,s,p,i,n: GLib.Variant(*props[n]), None)
menu = '''<node><interface name="com.canonical.dbusmenu">
<method name="GetLayout"><arg type="i" direction="in"/><arg type="i" direction="in"/><arg type="as" direction="in"/><arg type="u" direction="out"/><arg type="(ia{sv}av)" direction="out"/></method>
<method name="GetGroupProperties"><arg type="ai" direction="in"/><arg type="as" direction="in"/><arg type="a(ia{sv})" direction="out"/></method>
<method name="AboutToShow"><arg type="i" direction="in"/><arg type="b" direction="out"/></method>
<method name="Event"><arg type="i" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="in"/><arg type="u" direction="in"/></method>
<method name="EventGroup"><arg type="a(isvu)" direction="in"/><arg type="ai" direction="out"/></method>
<property name="Version" type="u" access="read"/><property name="TextDirection" type="s" access="read"/><property name="Status" type="s" access="read"/><property name="IconThemePath" type="as" access="read"/>
</interface></node>'''
mp = {'Version': ('u',3), 'TextDirection':('s','ltr'), 'Status':('s','normal'), 'IconThemePath':('as',[])}
bus.register_object('/Menu', Gio.DBusNodeInfo.new_for_xml(menu).interfaces[0], method, lambda c,s,p,i,n: GLib.Variant(*mp[n]), None)
bus.call_sync('org.kde.StatusNotifierWatcher','/StatusNotifierWatcher','org.kde.StatusNotifierWatcher','RegisterStatusNotifierItem', GLib.Variant('(s)',('/StatusNotifierItem',)), None, Gio.DBusCallFlags.NONE, -1,None)
GLib.MainLoop().run()
