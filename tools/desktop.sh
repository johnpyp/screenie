#!/usr/bin/env bash
# Headless GNOME and KDE Plasma sessions in containers (podman), for developing screenie
# on the desktops without wlroots' protocols. Each runs the real compositor (gnome-shell,
# kwin_wayland) with two virtual outputs, a D-Bus session, PipeWire and the desktop's
# portals, in a Fedora image (tools/desktop/*.Containerfile). The repository and cargo's
# target directory are mounted at the same paths, so a host build runs inside as is.
#
# Sessions: gnome and kde (Fedora 44: GNOME 50, Plasma 6.7), and gnome48 (Fedora 42),
# for what GNOME changed in 49.
#
# Usage (DESKTOP: gnome, gnome48 or kde):
#   tools/desktop.sh build [DESKTOP...]       # build the images (gnome and kde by default)
#   tools/desktop.sh start DESKTOP [ARG...]   # start a session (replacing a running one);
#                                             # ARGs go to the compositor (gnome-shell's
#                                             # --unsafe-mode allows org.gnome.Shell.Eval)
#   tools/desktop.sh stop DESKTOP
#   tools/desktop.sh run DESKTOP CMD...       # run a command in the session
#   tools/desktop.sh shot DESKTOP [FILE]      # screenshot of every output, side by side
#   tools/desktop.sh input DESKTOP CMD...     # pointer and keyboard (see tools/desktop/input.py)
#   tools/desktop.sh logs DESKTOP             # the compositor's log
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
  gnome | gnome48 | kde) echo "$1" ;;
  *)
    echo "which desktop: gnome, gnome48 or kde" >&2
    exit 1
    ;;
  esac
}

# The desktop a session runs.
kind() {
  case $1 in
  gnome*) echo gnome ;;
  *) echo "$1" ;;
  esac
}

# The Fedora release a session's image is built on.
fedora() {
  case $1 in
  gnome48) echo 42 ;;
  *) echo 44 ;;
  esac
}

build() {
  local name release
  for name in ${@:-gnome kde}; do
    name=$(desktop "$name")
    release=$(fedora "$name")
    podman build --build-arg "FEDORA=$release" -t "screenie-desktop-common:$release" \
      -f "$HERE/common.Containerfile" "$HERE"
    podman build --build-arg "COMMON=localhost/screenie-desktop-common:$release" \
      -t "screenie-desktop-$name" -f "$HERE/$(kind "$name").Containerfile" "$HERE"
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
  case $(kind "$name") in
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
    "$HERE/session.sh" "$(kind "$name")" "${@:2}" >/dev/null
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
start) start "$(desktop "${2:-}")" "${@:3}" ;;
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
  [[ $(kind "$name") == kde ]] && cargo build -q -p wlinput
  run "$name" python3 "$HERE/input.py" "$@"
  ;;
logs) podman logs "screenie-$(desktop "${2:-}")" 2>&1 | grep -v -E 'mod\.rt|RTKit|wp-internal|libcamera' ;;
*)
  sed -n '2,/^set -euo/p' "$0" | sed 's/^# \{0,1\}//;/^set -euo/d'
  exit 1
  ;;
esac
