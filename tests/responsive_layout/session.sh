#!/bin/bash
set -eu
export HOME=/work/home XDG_CONFIG_HOME=/work/config XDG_CACHE_HOME=/work/cache XDG_DATA_HOME=/work/data XDG_RUNTIME_DIR=/work/runtime TMPDIR=/tmp
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_DATA_HOME" "$XDG_RUNTIME_DIR" /tmp/.X11-unix /work/bin
chmod 700 "$XDG_RUNTIME_DIR"
export DISPLAY=:99 XDG_MENU_PREFIX=gnome- XDG_SESSION_TYPE=x11 XDG_CURRENT_DESKTOP=GNOME LIBGL_ALWAYS_SOFTWARE=1 GSETTINGS_BACKEND=dconf WINIT_X11_SCALE_FACTOR=1
Xvfb :99 -screen 0 2400x1600x24 -nolisten tcp >/work/xvfb.log 2>&1 &
echo $! >/work/xvfb.pid
sleep 1
dbus-update-activation-environment HOME XDG_CONFIG_HOME XDG_CACHE_HOME XDG_DATA_HOME XDG_RUNTIME_DIR DISPLAY XDG_MENU_PREFIX XDG_SESSION_TYPE XDG_CURRENT_DESKTOP LIBGL_ALWAYS_SOFTWARE
dbus-daemon --session --address=unix:path=/run/private-system-bus --fork --print-pid >/work/system-bus.pid
export DBUS_SYSTEM_BUS_ADDRESS=unix:path=/run/private-system-bus
gnome-shell --x11 --replace >/work/gnome.log 2>&1 &
echo $! >/work/gnome.pid
sleep 9
xdotool key Escape
python3 /harness/driver.py
