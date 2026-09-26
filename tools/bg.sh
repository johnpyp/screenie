#!/usr/bin/env bash
# Run and stop background processes by name, tracked with PID files (so killing one never
# pattern-matches the shell that asked for it).
#
#   tools/bg.sh start <name> <cmd...>   # stdout/stderr -> .cache/bg/<name>.log
#   tools/bg.sh stop <name>
#   tools/bg.sh log <name> [lines]
#   tools/bg.sh status <name>
set -euo pipefail

# Stop a process and wait for it to exit (SIGKILL after 3 s).
terminate() {
  local pid=$1
  kill "$pid" 2>/dev/null || return 0
  for _ in $(seq 30); do
    kill -0 "$pid" 2>/dev/null || return 0
    sleep 0.1
  done
  kill -9 "$pid" 2>/dev/null || true
}

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$ROOT/.cache/bg"
mkdir -p "$DIR"
name="${2:-}"
pidfile="$DIR/$name.pid"
case "${1:-}" in
start)
  shift 2
  [[ -f $pidfile ]] && terminate "$(cat "$pidfile")"
  setsid "$@" >"$DIR/$name.log" 2>&1 </dev/null &
  echo $! >"$pidfile"
  echo "started $name (pid $!)"
  ;;
stop)
  if [[ -f $pidfile ]]; then
    terminate "$(cat "$pidfile")"
    rm -f "$pidfile"
  fi
  ;;
log) tail -n "${3:-40}" "$DIR/$name.log" ;;
status)
  if [[ -f $pidfile ]] && kill -0 "$(cat "$pidfile")" 2>/dev/null; then echo running; else echo stopped; fi
  ;;
*)
  sed -n '2,8p' "$0"
  exit 1
  ;;
esac
