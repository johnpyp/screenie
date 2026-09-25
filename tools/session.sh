#!/usr/bin/env bash
# Headless Wayland test session for developing screenie on a machine without a display.
#
# Starts a headless sway with two outputs of different scales (the interesting case for
# coordinate math), a PipeWire stack, and the wlr portal, then writes the environment to
# $XDG_RUNTIME_DIR/screenie-session.env so other shells can `source` it.
#
# Usage:
#   tools/session.sh start [sway]   # start the compositor (default: sway)
#   tools/session.sh stop
#   tools/session.sh env            # print `export` lines for the running session
#   tools/session.sh shot [file]    # grim screenshot of the whole layout (default: session.png)
#   tools/session.sh run <cmd...>   # run a command inside the session environment
#
# Input can be driven with `wlrctl` (virtual pointer/keyboard), e.g.
#   tools/session.sh run wlrctl pointer move 100 100
set -euo pipefail

RUNTIME="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
STATE="$RUNTIME/screenie-session"
ENV_FILE="$RUNTIME/screenie-session.env"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

write_sway_config() {
  mkdir -p "$STATE"
  cat >"$STATE/sway.conf" <<EOF
# Two outputs: a 1x 1080p monitor on the left, a 1.5x 1440p monitor on the right.
output HEADLESS-1 resolution 1920x1080 position 0 0 scale 1 bg #3b4252 solid_color
output HEADLESS-2 resolution 2560x1440 position 1920 0 scale 1.5 bg #5e81ac solid_color
default_border pixel 2
font pango:Inter 10
seat seat0 fallback true
exec swaymsg create_output
exec sh -c 'printf "export WAYLAND_DISPLAY=%s\nexport SWAYSOCK=%s\n" "\$WAYLAND_DISPLAY" "\$SWAYSOCK" > "$STATE/compositor.env"'
EOF
}

start() {
  local compositor="${1:-sway}"
  if [[ -f $ENV_FILE ]] && pgrep -f "sway -c $STATE/sway.conf" >/dev/null; then
    echo "session already running; source $ENV_FILE"
    return 0
  fi
  rm -f "$STATE/compositor.env"
  case $compositor in
  sway)
    write_sway_config
    env -u WAYLAND_DISPLAY -u DISPLAY \
      WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER="${WLR_RENDERER:-gles2}" \
      XDG_CURRENT_DESKTOP=sway XDG_SESSION_TYPE=wayland \
      setsid sway -c "$STATE/sway.conf" >"$STATE/sway.log" 2>&1 </dev/null &
    ;;
  *)
    echo "unknown compositor: $compositor" >&2
    return 1
    ;;
  esac

  for _ in $(seq 50); do
    [[ -s $STATE/compositor.env ]] && break
    sleep 0.1
  done
  [[ -s $STATE/compositor.env ]] || {
    echo "compositor failed to start; see $STATE/sway.log" >&2
    tail -20 "$STATE/sway.log" >&2
    return 1
  }

  {
    cat "$STATE/compositor.env"
    echo "export XDG_RUNTIME_DIR=$RUNTIME"
    echo "export XDG_CURRENT_DESKTOP=sway"
    echo "export XDG_SESSION_TYPE=wayland"
    echo "export DBUS_SESSION_BUS_ADDRESS=unix:path=$RUNTIME/bus"
    echo "unset DISPLAY"
  } >"$ENV_FILE"

  # shellcheck disable=SC1090
  source "$ENV_FILE"
  systemctl --user import-environment WAYLAND_DISPLAY SWAYSOCK XDG_CURRENT_DESKTOP XDG_SESSION_TYPE
  systemctl --user start pipewire pipewire-pulse wireplumber 2>/dev/null || true
  # The portal caches the display it started on; restart it for this session.
  systemctl --user restart xdg-desktop-portal-wlr xdg-desktop-portal 2>/dev/null || true
  echo "session up: $(cat "$STATE/compositor.env" | tr '\n' ' ')"
  echo "source $ENV_FILE"
}

stop() {
  pkill -f "sway -c $STATE/sway.conf" 2>/dev/null || true
  rm -f "$ENV_FILE" "$STATE/compositor.env"
  echo "stopped"
}

need_session() {
  [[ -f $ENV_FILE ]] || {
    echo "no session running; tools/session.sh start" >&2
    exit 1
  }
  # shellcheck disable=SC1090
  source "$ENV_FILE"
}

case "${1:-}" in
start) start "${2:-sway}" ;;
stop) stop ;;
env) need_session && cat "$ENV_FILE" ;;
shot)
  need_session
  out="${2:-$ROOT/target/session.png}"
  mkdir -p "$(dirname "$out")"
  grim "$out" && echo "$out"
  ;;
run)
  need_session
  shift
  exec "$@"
  ;;
*)
  sed -n '2,/^set -euo/p' "$0" | sed 's/^# \{0,1\}//;/^set -euo/d'
  exit 1
  ;;
esac
