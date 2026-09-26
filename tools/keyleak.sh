#!/usr/bin/env bash
# Check that closing an overlay with the keyboard doesn't leak the key into the app
# beneath: when a surface closes on a key press, the compositor hands the still-held key
# to the next focused window, which then sees it held on enter and gets its release.
#
# Needs the headless session (tools/session.sh start) with a daemon running in it
# (tools/bg.sh start daemon ./target/release/screenie daemon), plus `wev`.
#
#   tools/keyleak.sh
#
# For each case it focuses a wev window, connects a virtual keyboard (before the overlay
# opens: one added mid-grab makes sway re-enter the focused toplevel, muddling the
# result), opens the overlay, presses the keys once it has focus, and checks what wev saw
# once focus comes back. Everything waits on events, not fixed sleeps.
set -euo pipefail
cd "$(dirname "$0")/.."
source "$XDG_RUNTIME_DIR/screenie-session.env"

screenie=./target/release/screenie
wlinput=./target/release/wlinput
cargo build -q --release -p screenie
cargo build -q --release --manifest-path tools/wlinput/Cargo.toml
dir=.cache/keyleak
mkdir -p "$dir"
log=$dir/wev.log

stdbuf -oL wev -f wl_keyboard:key -f wl_keyboard:enter -f wl_keyboard:leave > "$log" 2>&1 &
wev=$!
trap 'kill $wev 2>/dev/null; exec 3>&- 4<&- 2>/dev/null || true' EXIT

# until_line PATTERN AFTER-LINE: wait (up to 3 s) for a wev log line matching PATTERN past
# line AFTER-LINE, and print its line number.
until_line() {
  local n
  for _ in $(seq 150); do
    n=$(tail -n +$(($2 + 1)) "$log" | grep -nm1 -E "$1" | cut -d: -f1 || true)
    if [[ -n $n ]]; then
      echo $(($2 + n))
      return
    fi
    sleep 0.02
  done
  echo "timed out waiting for /$1/" >&2
  return 1
}

for _ in $(seq 150); do
  swaymsg -t get_tree | grep -q '"app_id": "wev"' && break
  sleep 0.02
done

failed=0
# case_ LABEL "WLINPUT COMMANDS" SCREENIE-ARGS...
case_() {
  local label=$1 keys=$2; shift 2
  swaymsg -q '[app_id="wev"] focus'
  rm -f "$dir/in" "$dir/out"
  mkfifo "$dir/in" "$dir/out"
  # shellcheck disable=SC2086
  $wlinput wait , $keys , wait < "$dir/in" > "$dir/out" &
  local input=$!
  exec 3> "$dir/in" 4< "$dir/out"
  read -r _ <&4 # connected
  local mark
  mark=$(wc -l < "$log")
  ("$screenie" "$@" > /dev/null 2>&1 &)
  local verdict
  if mark=$(until_line "leave" "$mark"); then # the overlay has the keyboard
    echo >&3
    read -r _ <&4 # keys sent
    if mark=$(until_line "enter" "$mark"); then # the overlay closed; focus is back
      sleep 0.05 # a leaked release follows the enter immediately
      # Keys still held show as `sym:` lines under the enter; their release as `key:`.
      local seen
      seen=$(tail -n +"$mark" "$log" | grep -E "key:|^ +sym" || true)
      if [[ -z $seen ]]; then
        verdict="PASS  $label"
      else
        verdict="FAIL  $label"$'\n'"$(sed 's/^/        /' <<< "$seen")"
        failed=1
      fi
    else
      verdict="FAIL  $label (never closed)"
      failed=1
    fi
  else
    verdict="FAIL  $label (never opened)"
    failed=1
  fi
  echo >&3 || true
  exec 3>&- 4<&-
  wait $input 2>/dev/null || true
  echo "$verdict"
}

case_ "editor: Esc closes" "key escape" shot -r "200,200 600x400" --edit
case_ "editor: Enter (Done) closes" "key enter" shot -r "200,200 600x400" --edit
case_ "editor: Ctrl+Q closes" "hold ctrl , key q , release ctrl" shot -r "200,200 600x400" --edit
case_ "selector: Esc cancels" "key escape" shot
case_ "selector: Enter captures" "key enter" shot --no-preview
exit $failed
