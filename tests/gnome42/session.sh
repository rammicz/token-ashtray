#!/bin/bash
set -eu
export HOME=/work/home XDG_CONFIG_HOME=/work/config XDG_CACHE_HOME=/work/cache XDG_DATA_HOME=/work/data XDG_RUNTIME_DIR=/work/runtime TMPDIR=/tmp
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_DATA_HOME" "$XDG_RUNTIME_DIR" /tmp/.X11-unix
chmod 700 "$XDG_RUNTIME_DIR"
export DISPLAY=:99 XDG_SESSION_TYPE=x11 XDG_CURRENT_DESKTOP=GNOME LIBGL_ALWAYS_SOFTWARE=1 GSETTINGS_BACKEND=dconf
if [ -f /work/route.zip ]; then
 mkdir -p "$XDG_DATA_HOME/gnome-shell/extensions/ubuntu-appindicators@ubuntu.com"
 unzip -q /work/route.zip -d "$XDG_DATA_HOME/gnome-shell/extensions/ubuntu-appindicators@ubuntu.com"
fi
Xvfb :99 -screen 0 1280x1024x24 -nolisten tcp >/work/xvfb.log 2>&1 &
sleep 1
dbus-update-activation-environment HOME XDG_CONFIG_HOME XDG_CACHE_HOME XDG_DATA_HOME XDG_RUNTIME_DIR DISPLAY XDG_SESSION_TYPE XDG_CURRENT_DESKTOP LIBGL_ALWAYS_SOFTWARE
gsettings set org.gnome.shell disable-user-extensions false
gsettings set org.gnome.shell enabled-extensions "['ubuntu-appindicators@ubuntu.com']"
dbus-daemon --session --address=unix:path=/run/private-system-bus --fork --print-pid >/work/system-bus.pid
export DBUS_SYSTEM_BUS_ADDRESS=unix:path=/run/private-system-bus
gnome-shell --x11 --replace >/work/gnome.log 2>&1 &
for i in $(seq 1 30); do
 if gdbus call --session --dest org.kde.StatusNotifierWatcher --object-path /StatusNotifierWatcher --method org.freedesktop.DBus.Properties.Get org.kde.StatusNotifierWatcher IsStatusNotifierHostRegistered >/work/watcher.txt 2>/work/watcher-error.txt; then
  sleep 3
  xdotool key Super_L
  sleep 2
  # Enable evaluation only in this disposable Shell to inspect actor geometry.
  xdotool key alt+F2
  sleep 1
  xdotool type lg
  xdotool key Return
  sleep 2
  xdotool type 'global.context.unsafe_mode = true'
  xdotool key Return
  sleep 1
  xdotool key Escape
  sleep 1
  gnome-extensions info ubuntu-appindicators@ubuntu.com >/work/extension.txt
  gnome-shell --version >/work/shell-version.txt
  python3 /harness/driver.py
  exit $?
 fi
 sleep 1
done
exit 2
