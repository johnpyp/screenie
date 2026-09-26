#!/usr/bin/env bash
# A headless demo desktop for screenie's screenshots and video: one 4K output at 2x,
# Catppuccin Mocha, waybar, translucent foot terminals, a generated wallpaper.
#
# Everything lives under .cache/demo: the home directory it runs with (copied from
# tools/demo/home on every start), the downloaded cursor theme and font, the wallpaper.
# Your own config is never read.
#
# Usage:
#   tools/demo/desktop.sh start     # compositor, bar, then the windows
#   tools/demo/desktop.sh stop
#   tools/demo/desktop.sh env       # `export` lines for the running desktop
#   tools/demo/desktop.sh shot [f]  # grim screenshot (default .cache/demo/desktop.png)
#   tools/demo/desktop.sh run CMD…  # run a command in the desktop's environment
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
DEMO="$ROOT/.cache/demo"
ASSETS="$DEMO/assets"
HOME_DIR="$DEMO/home"
RUNTIME="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
STATE="$RUNTIME/screenie-demo"
ENV_FILE="$RUNTIME/screenie-demo.env"

assets() {
  mkdir -p "$ASSETS"
  [[ -f $ASSETS/bibata.tar.xz ]] || curl -sSLf -o "$ASSETS/bibata.tar.xz" \
    https://github.com/ful1e5/Bibata_Cursor/releases/latest/download/Bibata-Modern-Ice.tar.xz
  [[ -f $ASSETS/jbm.tar.xz ]] || curl -sSLf -o "$ASSETS/jbm.tar.xz" \
    https://github.com/ryanoasis/nerd-fonts/releases/latest/download/JetBrainsMono.tar.xz
  [[ -f $ASSETS/wallpaper.png ]] || uv run -q --script "$ROOT/tools/demo/wallpaper.py" "$ASSETS/wallpaper.png"
}

prepare_home() {
  rm -rf "$HOME_DIR"
  mkdir -p "$HOME_DIR/.local/share/icons" "$HOME_DIR/.local/share/fonts/JetBrainsMonoNerd" \
    "$HOME_DIR/.local/bin" "$HOME_DIR/Pictures" "$HOME_DIR/Videos"
  cp -a "$ROOT/tools/demo/home/." "$HOME_DIR/"
  tar -xJf "$ASSETS/bibata.tar.xz" -C "$HOME_DIR/.local/share/icons"
  tar -xJf "$ASSETS/jbm.tar.xz" -C "$HOME_DIR/.local/share/fonts/JetBrainsMonoNerd" \
    --wildcards 'JetBrainsMonoNerdFont-*.ttf'
  ln -sf "$ROOT/target/release/screenie" "$HOME_DIR/.local/bin/screenie"
  sed -i "s|@WALLPAPER@|$ASSETS/wallpaper.png|; s|@STATE@|$STATE|" "$HOME_DIR/.config/sway/config"
}

# The environment every program on the desktop gets.
demo_env() {
  echo "export HOME=$HOME_DIR"
  echo "export XDG_CONFIG_HOME=$HOME_DIR/.config"
  echo "export XDG_DATA_HOME=$HOME_DIR/.local/share"
  echo "export XDG_STATE_HOME=$HOME_DIR/.local/state"
  echo "export XDG_CACHE_HOME=$HOME_DIR/.cache"
  echo "export XDG_RUNTIME_DIR=$RUNTIME"
  echo "export XDG_CURRENT_DESKTOP=sway"
  echo "export XDG_SESSION_TYPE=wayland"
  echo "export XCURSOR_PATH=$HOME_DIR/.local/share/icons:/usr/share/icons"
  echo "export XCURSOR_THEME=Bibata-Modern-Ice"
  echo "export XCURSOR_SIZE=24"
  echo "export PATH=$HOME_DIR/.local/bin:$PATH"
  echo "export SCREENIE_LOG=info"
  echo "unset DISPLAY"
}

start() {
  if [[ -f $ENV_FILE ]] && pgrep -f "sway -c $HOME_DIR/.config/sway/config" >/dev/null; then
    echo "desktop already running; source $ENV_FILE"
    return 0
  fi
  [[ -x $ROOT/target/release/screenie ]] || cargo build --release -p screenie
  assets
  prepare_home
  mkdir -p "$STATE"
  rm -f "$STATE/compositor.env"
  (
    eval "$(demo_env)"
    env -u WAYLAND_DISPLAY WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=gles2 \
      setsid sway -c "$HOME_DIR/.config/sway/config" >"$STATE/sway.log" 2>&1 </dev/null &
  )
  for _ in $(seq 50); do
    [[ -s $STATE/compositor.env ]] && break
    sleep 0.1
  done
  [[ -s $STATE/compositor.env ]] || {
    echo "sway failed to start; see $STATE/sway.log" >&2
    tail -20 "$STATE/sway.log" >&2
    return 1
  }
  { cat "$STATE/compositor.env"; demo_env; } >"$ENV_FILE"
  windows
  echo "desktop up; source $ENV_FILE"
}

# The layout: fastfetch, the app's logs and files on the left; the code over btop
# on the right.
windows() {
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  local dir=$HOME_DIR/code/acme-api
  swaymsg -q "exec foot --app-id=term-fetch --title='~/code/acme-api' -D $dir demo-intro"
  sleep 0.6
  swaymsg -q "splith"
  swaymsg -q "exec foot --app-id=term-code --title='nvim src/billing.rs' -D $dir nvim src/billing.rs"
  sleep 0.6
  swaymsg -q "splitv"
  swaymsg -q "exec foot --app-id=term-btop --title=btop btop"
  sleep 0.6
  swaymsg -q '[app_id="term-code"] resize set height 62 ppt'
  swaymsg -q '[app_id="term-fetch"] resize set width 42 ppt'
  swaymsg -q '[app_id="term-fetch"] focus'
}

stop() {
  pkill -f "sway -c $HOME_DIR/.config/sway/config" 2>/dev/null || true
  rm -f "$ENV_FILE" "$STATE/compositor.env"
  echo "stopped"
}

need_desktop() {
  [[ -f $ENV_FILE ]] || {
    echo "no demo desktop running; tools/demo/desktop.sh start" >&2
    exit 1
  }
  # shellcheck disable=SC1090
  source "$ENV_FILE"
}

case "${1:-}" in
start) start ;;
stop) stop ;;
windows) need_desktop && windows ;;
env) need_desktop && cat "$ENV_FILE" ;;
shot)
  need_desktop
  out="${2:-$DEMO/desktop.png}"
  grim "$out" && echo "$out"
  ;;
run)
  need_desktop
  shift
  exec "$@"
  ;;
*)
  sed -n '2,/^set -euo/p' "$0" | sed 's/^# \{0,1\}//;/^set -euo/d'
  exit 1
  ;;
esac
