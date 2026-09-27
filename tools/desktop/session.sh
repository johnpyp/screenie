#!/bin/bash
# The session inside a desktop image (see tools/desktop.sh): D-Bus, PipeWire, then the
# compositor with two virtual outputs.
set -u
# The compositors want a system bus to talk to; nothing answers on this one.
dbus-daemon --session --address="$DBUS_SYSTEM_BUS_ADDRESS" --nofork --nopidfile --syslog-only &
dbus-daemon --session --address="$DBUS_SESSION_BUS_ADDRESS" --nofork --nopidfile --syslog-only &
for _ in $(seq 50); do
  [ -S "$XDG_RUNTIME_DIR/bus" ] && [ -S "$XDG_RUNTIME_DIR/system_bus" ] && break
  sleep 0.1
done
# What D-Bus-activated services (the portals) start with.
dbus-update-activation-environment XDG_CURRENT_DESKTOP XDG_SESSION_TYPE XDG_RUNTIME_DIR \
  HOME DBUS_SYSTEM_BUS_ADDRESS WAYLAND_DISPLAY KDE_FULL_SESSION KDE_SESSION_VERSION
pipewire &
sleep 0.3
wireplumber &
case $1 in
gnome)
  exec gnome-shell --headless --wayland --no-x11 \
    --virtual-monitor 1920x1080 --virtual-monitor 2560x1440
  ;;
kde)
  exec kwin_wayland --virtual --no-lockscreen --socket wayland-0 \
    --width 1920 --height 1080 --output-count 2
  ;;
esac
