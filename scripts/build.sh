#!/usr/bin/env bash
# Safe build script for aeordb-client.
# Limits parallel jobs to avoid OOM kills that destroy other processes.
set -euo pipefail

JOBS="${AEORDB_BUILD_JOBS:-2}"

echo "Building aeordb-client with -j $JOBS (override with AEORDB_BUILD_JOBS=N)"
cargo build -j "$JOBS" "$@"
