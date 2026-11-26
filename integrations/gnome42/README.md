# GNOME 42 Token Ashtray host route

This is an optional, separate GNOME Shell extension distribution. It is not
linked into Token Ashtray and is not included in its application binary ZIP.
The application installer does not install or enable it.

Supported baseline: Ubuntu GNOME Shell 42.9 (X11) and Ubuntu AppIndicators
`42-2~fakesync1`, UUID `ubuntu-appindicators@ubuntu.com`. No claim is made for
other Shell releases, Wayland pointer behavior, or native Windows.

The route changes only SNI Id `token-ashtray`: one left press invokes Activate
without opening a menu. Subsequent presses in a multi-click gesture do not
invoke it again. Continuations use the Shell’s configured double-click time and
distance, measured from the previous press; another button resets the gesture.
Right-click keeps the upstream Shell menu and DBusMenu actions.
Keyboard, touch, middle-click, scrolling, XEmbed and unrelated SNI items keep
upstream routing. Menu-open notification never invokes Activate. The application
behavior behind Activate (opening the board) and close-to-tray belong to the app.

## Source and license

Upstream: https://github.com/ubuntu/gnome-shell-extension-appindicator
Ubuntu package: `gnome-shell-extension-appindicator_42-2~fakesync1_all.deb`,
SHA256 `42cbf2afea3f290e36bdab54d75214a94490af2a49d66c5314c5b8aee93e6ab6`,
Ubuntu archive path
`pool/main/g/gnome-shell-extension-appindicator/gnome-shell-extension-appindicator_42-2~fakesync1_all.deb`.
`inputs.json` pins every consumed installed source, interface, schema and notice.
The builder refuses different bytes. The installed baseline must be available;
it performs no network fetch and does not change system files.

Upstream extension sources and this extension modification are GPL-2.0-or-later.
Copyright 2011 Giovanni Campagna, 2013–2014 Jonas Kümmerlin and the additional
contributors identified in the source headers. The archive includes the complete
editable extension JS and interface XML, schema source, upstream COPYRIGHT,
GPL text in COPYING, the GPL build script, this provenance, and input hashes. Rebuild its schema with
`glib-compile-schemas --strict schemas`. Preserve these files when redistributing
the extension; recipients retain GPL rights independently of the app's license.
Keep application sources outside this separate extension archive; the MIT app
and GPL extension retain their respective licenses.

## Build and isolated verification

Requires Python 3, GLib schema compiler and the pinned installed Ubuntu package:

```sh
python3 integrations/gnome42/build.py /tmp/token-ashtray-gnome42.zip
```

An extracted Debian package may be used with `--source-root PATH`; provide the
pinned `/usr/share/common-licenses/GPL-2` beneath that root too. Output is a
deterministic ZIP containing runnable sources, not a compiled app dependency.

Native test dependencies: bubblewrap, Xvfb, GNOME Shell 42.9, the pinned extension,
D-Bus tools, Python GI, xdotool and Gdk 3 introspection. Run from this repository:

```sh
python3 tests/gnome42/run.py /tmp/gnome42-stock-new
python3 tests/gnome42/run.py /tmp/gnome42-route-new --route /tmp/token-ashtray-gnome42.zip
```

Each work path must be new. The first command is expected to fail the single-left
Activate assertion against stock routing. The second starts with a fresh profile
and installs the ZIP only there. Both run native X11 pointer input through real
GNOME Shell, with real SNI and DBusMenu fixtures. They use virtual Xvfb pointer
events, not a human hardware mouse. The fixture logs dispatch separately from
rendered menu captures. No provider account or app-window lifecycle is tested.
Bubblewrap provides private PID, mount, network, IPC, display, system/session bus,
HOME and XDG state; namespace teardown removes the owned session. No host desktop
or other runtime is reused. No TCP listener is started.

## Optional manual installation and removal

Installation in an actual desktop is a separate operator choice. Back up any
existing per-user `ubuntu-appindicators@ubuntu.com` directory first. Extract the
ZIP into `$XDG_DATA_HOME/gnome-shell/extensions/ubuntu-appindicators@ubuntu.com`
(default `~/.local/share/gnome-shell/extensions/ubuntu-appindicators@ubuntu.com`).
It overrides the system copy for that user, retaining the same UUID so two
watchers are not enabled simultaneously. Enable that UUID using Extensions and
log out/in. Confirm the loaded path and ENABLED state with
`gnome-extensions info ubuntu-appindicators@ubuntu.com`.

To revert, remove only the directory installed above (or restore its backup),
then log out/in; the system package remains intact. System extension upgrades
will not update this per-user override: remove/revalidate it when upgrading
Shell. Never run this procedure on another user's active desktop as a test.

## Handoff

Native acceptance of the tray integration needs the exact archive hash, loaded Shell/extension identity, native
pointer dispatch logs, menu screenshots, selected actions and cleanup evidence
from the verification run. This host route alone does not settle the remaining
app-window acceptance, native Windows, live-provider evidence or delivery.
