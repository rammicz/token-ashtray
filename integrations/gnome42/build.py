# SPDX-License-Identifier: GPL-2.0-or-later
"""Build a separate GPL-2.0-or-later GNOME extension source archive.

Does not install, enable, or modify any desktop. Only accepts the pinned inputs.
"""
import argparse
import hashlib
import json
import subprocess
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent
UUID = 'ubuntu-appindicators@ubuntu.com'
PATCH = '''    // Token Ashtray host route, 2026-11-18. GPL-2.0-or-later.
    // Intercept the generic PanelMenu event BEFORE its menu toggle. Suppress
    // subsequent presses in the same multi-click gesture; keyboard/touch and
    // all other indicators keep the upstream implementation.
    vfunc_event(event) {
        if (this._indicator.id === 'token-ashtray' &&
            event.type() === Clutter.EventType.BUTTON_PRESS &&
            event.get_button() === 1) {
            const [x, y] = event.get_coords();
            const time = event.get_time();
            const settings = Clutter.Settings.get_default();
            const previous = this._tokenAshtrayPress;
            const continuation = previous &&
                ((time - previous.time) >>> 0) <= settings.double_click_time &&
                Math.abs(x - previous.x) <= settings.double_click_distance &&
                Math.abs(y - previous.y) <= settings.double_click_distance;
            this._tokenAshtrayPress = {time, x, y};
            if (!continuation)
                this._indicator.open(x, y);
            return Clutter.EVENT_STOP;
        }
        if (event.type() === Clutter.EventType.BUTTON_PRESS)
            this._tokenAshtrayPress = null;
        return super.vfunc_event(event);
    }

'''

def build(source, output):
    manifest = json.loads((ROOT/'inputs.json').read_text())
    files = {}
    for relative, digest in manifest.items():
        data = (source/relative).read_bytes()
        if hashlib.sha256(data).hexdigest() != digest:
            raise ValueError('Unsupported input: ' + relative)
        files[relative] = data
    base = 'usr/share/gnome-shell/extensions/' + UUID + '/'
    payload = {name.removeprefix(base): data for name,data in files.items() if name.startswith(base)}
    name = 'indicatorStatusIcon.js'
    text = payload[name].decode()
    marker = '    vfunc_button_press_event(buttonEvent) {'
    assert text.count(marker) == 1
    payload[name] = text.replace(marker, PATCH+marker).encode()
    metadata = json.loads(payload['metadata.json'])
    metadata['shell-version'] = ['42']
    metadata['description'] += ' Token Ashtray single-left host route (2026-11-18).'
    payload['metadata.json'] = (json.dumps(metadata,indent=2)+'\n').encode()
    payload['schemas/org.gnome.shell.extensions.appindicator.gschema.xml'] = files['usr/share/glib-2.0/schemas/org.gnome.shell.extensions.appindicator.gschema.xml']
    payload['COPYING'] = files['usr/share/common-licenses/GPL-2']
    payload['COPYRIGHT'] = files['usr/share/doc/gnome-shell-extension-appindicator/copyright']
    payload['README.txt'] = (ROOT/'README.md').read_bytes()
    payload['inputs.json'] = (ROOT/'inputs.json').read_bytes()
    payload['build.py'] = Path(__file__).read_bytes()
    # Complete editable JS/XML sources travel with the extension; schema output
    # can be rebuilt with the command below, without any application sources.
    with tempfile.TemporaryDirectory() as tmp:
        schemas = Path(tmp)/'schemas'
        schemas.mkdir()
        (schemas/'org.gnome.shell.extensions.appindicator.gschema.xml').write_bytes(payload['schemas/org.gnome.shell.extensions.appindicator.gschema.xml'])
        subprocess.run(['glib-compile-schemas','--strict',str(schemas)],check=True)
        payload['schemas/gschemas.compiled'] = (schemas/'gschemas.compiled').read_bytes()
    with zipfile.ZipFile(output,'w',zipfile.ZIP_DEFLATED) as archive:
        for name,data in sorted(payload.items()):
            info = zipfile.ZipInfo(name, (2026,11,18,0,0,0))
            info.external_attr = 0o100644 << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info,data)
    print(hashlib.sha256(output.read_bytes()).hexdigest(),output)

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output',type=Path)
    parser.add_argument('--source-root',type=Path,default=Path('/'))
    args = parser.parse_args()
    build(args.source_root,args.output)
