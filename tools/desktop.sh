#!/usr/bin/env bash
# Headless GNOME and KDE Plasma sessions in containers (podman), for developing screenie
# on the desktops without wlroots' protocols. Each runs the real compositor (gnome-shell,
# kwin_wayland) with two virtual outputs, a D-Bus session, PipeWire and the desktop's
# portals, in a Fedora image (tools/desktop/*.Containerfile). The repository and cargo's
# target directory are mounted at the same paths, so a host build runs inside as is.
#
# Usage:
#   tools/desktop.sh build [gnome|kde]      # build the images (both by default)
#   tools/desktop.sh start gnome|kde        # start a session (replacing a running one)
#   tools/desktop.sh stop gnome|kde
#   tools/desktop.sh run gnome|kde CMD...   # run a command in the session
#   tools/desktop.sh shot gnome|kde [FILE]  # screenshot of every output, side by side
#   tools/desktop.sh input gnome|kde CMD... # pointer and keyboard (see tools/desktop/input.py)
#   tools/desktop.sh logs gnome|kde         # the compositor's log
#
# The session's home is .cache/desktop/<name>/home, kept between runs (delete it to start
# afresh); screenie's daemon log is under it. The runtime directory (Wayland, D-Bus and
# PipeWire sockets) is $XDG_RUNTIME_DIR/screenie-<name> on the host.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HERE="$ROOT/tools/desktop"
RUNTIME="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"

desktop() {
  case "${1:-}" in
  gnome | kde) echo "$1" ;;
  *)
    echo "which desktop: gnome or kde" >&2
    exit 1
    ;;
  esac
}

build() {
  podman build -t screenie-desktop-common -f "$HERE/common.Containerfile" "$HERE"
  for d in "${@:-gnome kde}"; do
    for name in $d; do
      podman build -t "screenie-desktop-$name" -f "$HERE/$name.Containerfile" "$HERE"
    done
  done
}

start() {
  local name=$1 rundir="$RUNTIME/screenie-$1" home="$ROOT/.cache/desktop/$1/home"
  podman rm -f "screenie-$name" >/dev/null 2>&1 || true
  rm -rf "$rundir"
  mkdir -p "$rundir" "$home"
  chmod 700 "$rundir"
  # cargo's target directory may be a link out of the repository.
  local target
  target="$(readlink -f "$ROOT/target")"
  local env=(
    -e XDG_RUNTIME_DIR=/run/user/1000 -e HOME="$home" -e XDG_SESSION_TYPE=wayland
    -e WAYLAND_DISPLAY=wayland-0
    -e DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus
    -e DBUS_SYSTEM_BUS_ADDRESS=unix:path=/run/user/1000/system_bus
  )
  case $name in
  gnome) env+=(-e XDG_CURRENT_DESKTOP=GNOME) ;;
  kde) env+=(-e XDG_CURRENT_DESKTOP=KDE -e KDE_FULL_SESSION=true -e KDE_SESSION_VERSION=6) ;;
  esac
  # The host's GPU, where there is one: KWin renders without, but takes screenshots
  # only with OpenGL.
  local gpu=()
  [[ -d /dev/dri ]] && gpu=(--device /dev/dri)
  podman run -d --name "screenie-$name" --userns=keep-id --user "$(id -u):$(id -g)" \
    -v "$ROOT:$ROOT" -v "$target:$target:ro" -v "$rundir:/run/user/1000" \
    "${gpu[@]}" --shm-size=1g "${env[@]}" "localhost/screenie-desktop-$name" \
    "$HERE/session.sh" "$name" >/dev/null
  for _ in $(seq 100); do
    [[ -S $rundir/wayland-0 ]] && break
    sleep 0.1
  done
  [[ -S $rundir/wayland-0 ]] || {
    podman logs "screenie-$name" 2>&1 | tail -20 >&2
    echo "$name didn't start" >&2
    exit 1
  }
  # The desktop sets its outputs up once it's running.
  run "$name" python3 "$HERE/outputs.py"
  echo "$name session up: tools/desktop.sh run $name CMD..."
}

run() {
  local name=$1
  shift
  # A terminal, or at least stdin (commands piped to `input -`).
  local tty=(-i)
  [[ -t 0 ]] && tty=(-it)
  podman exec "${tty[@]}" -w "$PWD" "screenie-$name" "$@"
}

case "${1:-}" in
build)
  shift
  build "$@"
  ;;
start) start "$(desktop "${2:-}")" ;;
stop) podman rm -f "screenie-$(desktop "${2:-}")" >/dev/null && echo stopped ;;
run)
  name=$(desktop "${2:-}")
  shift 2
  run "$name" "$@"
  ;;
shell) run "$(desktop "${2:-}")" bash ;;
shot)
  name=$(desktop "${2:-}")
  out="${3:-$ROOT/.cache/desktop/$name/screen.png}"
  mkdir -p "$(dirname "$out")"
  run "$name" python3 "$HERE/shot.py" "$(realpath -m "$out")"
  ;;
input)
  name=$(desktop "${2:-}")
  shift 2
  # KWin's input goes through wlinput.
  [[ $name == kde ]] && cargo build -q -p wlinput
  run "$name" python3 "$HERE/input.py" "$@"
  ;;
logs) podman logs "screenie-$(desktop "${2:-}")" 2>&1 | grep -v -E 'mod\.rt|RTKit|wp-internal|libcamera' ;;
*)
  sed -n '2,/^set -euo/p' "$0" | sed 's/^# \{0,1\}//;/^set -euo/d'
  exit 1
  ;;
esac
