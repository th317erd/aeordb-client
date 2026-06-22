#!/usr/bin/env bash
# Gracefully stop the aeordb-client process.
# Tries the API shutdown endpoint first, then SIGTERM, never SIGKILL.
set -euo pipefail

PORT="${AEORDB_CLIENT_PORT:-9400}"
TIMEOUT=5

echo "Attempting graceful shutdown via API (port $PORT)..."
if curl -sf -X POST "http://127.0.0.1:$PORT/api/v1/shutdown" --max-time "$TIMEOUT" >/dev/null 2>&1; then
  echo "Shutdown request sent. Waiting for process to exit..."
  sleep 2
  if ! curl -sf "http://127.0.0.1:$PORT/api/v1/status" --max-time 2 >/dev/null 2>&1; then
    echo "Client stopped."
    exit 0
  fi
fi

# API didn't work — find the process and SIGTERM it
echo "API shutdown failed or timed out. Sending SIGTERM..."
PIDS=$(pgrep -f '(^|/|[[:space:]])(target/debug/)?aeordb-client([[:space:]]|$)' 2>/dev/null || true)
if [ -z "$PIDS" ]; then
  echo "No aeordb-client process found."
  exit 0
fi

for PID in $PIDS; do
  echo "  Sending SIGTERM to PID $PID"
  kill -TERM "$PID" 2>/dev/null || true
done

# Wait for graceful exit
for i in $(seq 1 10); do
  if ! pgrep -f '(^|/|[[:space:]])(target/debug/)?aeordb-client([[:space:]]|$)' >/dev/null 2>&1; then
    echo "Client stopped."
    exit 0
  fi
  sleep 1
done

echo "WARNING: Client still running after 10s. NOT sending SIGKILL — investigate manually."
echo "  Remaining PIDs: $(pgrep -f '(^|/|[[:space:]])(target/debug/)?aeordb-client([[:space:]]|$)' 2>/dev/null || echo 'none')"
exit 1
