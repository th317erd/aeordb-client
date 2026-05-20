#!/usr/bin/env bash
#
# dev-watch.sh — Watch the aeordb-client binary and auto-restart on change.
#
# Usage: ./dev-watch.sh
# Stop:  Ctrl+C
#
# Per CLAUDE.md project policy this script never escalates to SIGKILL.
# If the running instance ignores SIGTERM after 5s, dev-watch logs a
# warning and stops trying — the user is expected to investigate the
# stuck process manually rather than have a tooling script murder it.

set -euo pipefail

BINARY="./target/debug/aeordb-client"
SERVER_URL="http://127.0.0.1:9400"
LOG="/tmp/aeordb-client.log"
PID=""

cleanup() {
  if [[ -n "$PID" ]] && kill -0 "$PID" 2>/dev/null; then
    echo "[dev-watch] Stopping aeordb-client (PID $PID)..."
    kill -TERM "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  exit 0
}

trap cleanup EXIT INT TERM

stop_old() {
  if [[ -z "$PID" ]] || ! kill -0 "$PID" 2>/dev/null; then
    return 0
  fi
  echo "[dev-watch] Stopping old instance (PID $PID)..."
  # Prefer API shutdown — fastest for healthy processes.
  curl -sf -X POST "$SERVER_URL/api/v1/shutdown" --max-time 2 &>/dev/null || true
  # SIGTERM if the API path didn't take it down.
  kill -TERM "$PID" 2>/dev/null || true
  # Wait up to 5s for graceful exit. No SIGKILL escalation per project policy.
  for _ in 1 2 3 4 5; do
    kill -0 "$PID" 2>/dev/null || return 0
    sleep 1
  done
  echo "[dev-watch] WARNING: PID $PID still alive after 5s. Skipping restart — investigate manually."
  return 1
}

start_app() {
  stop_old || return 1

  echo "[dev-watch] Starting aeordb-client..."
  # setsid + disown puts the child in its own session so a Ctrl+C in this
  # terminal hits dev-watch (we re-broadcast via the trap) and doesn't
  # double-signal the child mid-shutdown.
  setsid "$BINARY" &>"$LOG" &
  PID=$!
  disown "$PID" 2>/dev/null || true
  echo "[dev-watch] Started (PID $PID) — logs at $LOG"

  # Wait up to 15s for the HTTP server to come up.
  for _ in $(seq 1 15); do
    if curl -sf "$SERVER_URL/api/v1/status" &>/dev/null; then
      echo "[dev-watch] Server ready"
      return 0
    fi
    sleep 1
  done
  echo "[dev-watch] WARNING: HTTP not reachable after 15s — check $LOG"
}

# Initial build (-j 2 is mandatory — see CLAUDE.md) and start.
if [[ ! -f "$BINARY" ]]; then
  echo "[dev-watch] Binary not found, building (cargo build -j 2)..."
  cargo build -j 2
fi

start_app

echo "[dev-watch] Watching $BINARY for changes... (Ctrl+C to stop)"

# Detect rebuilds by binary fingerprint (mtime + size).
get_fingerprint() {
  stat -c '%Y_%s' "$BINARY" 2>/dev/null || echo 0
}

LAST_FP=$(get_fingerprint)

while true; do
  sleep 1

  if [[ ! -f "$BINARY" ]]; then
    continue
  fi

  CURRENT_FP=$(get_fingerprint)

  if [[ "$CURRENT_FP" != "$LAST_FP" ]]; then
    # Wait a beat for the cargo writer to finish flushing.
    sleep 1
    CURRENT_FP=$(get_fingerprint)
    echo ""
    echo "[dev-watch] Binary changed — restarting..."
    LAST_FP="$CURRENT_FP"
    start_app
  fi

  # Restart if the process died on its own.
  if [[ -n "$PID" ]] && ! kill -0 "$PID" 2>/dev/null; then
    echo "[dev-watch] Process died — restarting..."
    start_app
  fi
done
