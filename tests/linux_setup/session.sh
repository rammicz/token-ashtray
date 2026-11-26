#!/bin/bash
set -eu
export HOME='/work/home profile' XDG_CONFIG_HOME='/work/config path' XDG_CACHE_HOME='/work/cache path' XDG_DATA_HOME='/work/data path' XDG_RUNTIME_DIR=/work/runtime TMPDIR=/tmp
mkdir -p "$HOME/Downloads" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_DATA_HOME" "$XDG_RUNTIME_DIR" /tmp/.X11-unix
if [ ! -f /work/absent-applications ]; then mkdir -p "$XDG_DATA_HOME/applications"; fi
chmod 700 "$XDG_RUNTIME_DIR"
export XDG_MENU_PREFIX=gnome-
export DISPLAY=:99 XDG_SESSION_TYPE=x11 XDG_CURRENT_DESKTOP=GNOME LIBGL_ALWAYS_SOFTWARE=1 GSETTINGS_BACKEND=dconf
if [ ! -f /work/resume-profile ]; then
 cp /work/download "$HOME/Downloads/token-ashtray-setup-linux-x86_64"
 chmod 644 "$HOME/Downloads/token-ashtray-setup-linux-x86_64"
fi
Xvfb :99 -screen 0 1280x1024x24 -nolisten tcp >/work/xvfb.log 2>&1 &
echo $! >/work/xvfb.pid
sleep 1
dbus-update-activation-environment HOME XDG_CONFIG_HOME XDG_CACHE_HOME XDG_DATA_HOME XDG_RUNTIME_DIR DISPLAY XDG_MENU_PREFIX XDG_SESSION_TYPE XDG_CURRENT_DESKTOP LIBGL_ALWAYS_SOFTWARE
# All buses, desktops, sockets and processes are private to this namespace.
dbus-daemon --session --address=unix:path=/run/private-system-bus --fork --print-pid >/work/system-bus.pid
export DBUS_SYSTEM_BUS_ADDRESS=unix:path=/run/private-system-bus
gsettings set org.gnome.desktop.search-providers disable-external true
gnome-shell --x11 --replace >/work/gnome.log 2>&1 &
echo $! >/work/gnome.pid
sleep 10
python3 /harness/driver.py
