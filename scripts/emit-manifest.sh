#!/usr/bin/env bash
#
# Emit manifest.json + manifest.sig.json next to the release binaries in
# aeordb-www/downloads/.
#
# Run this AFTER the build pipeline has placed all artifacts in the
# downloads/ directory. The manifest carries every field needed to verify
# and install the update — version, timestamp, file names, file sizes, and
# sha256 hashes — and is signed offline with the AEOR ed25519 private key
# so aeordb-www cannot tamper with the URL/hash a client receives.
#
# Signature shape (manifest.sig.json):
#
#   { "key_id":    "aeor-<YYYYMMDDHHMM>",
#     "signature": "<base64-of-64-byte-ed25519>" }
#
# Usage:
#   scripts/emit-manifest.sh --key /secure/aeor-202605132323-private-key.bin
#
# The private key argument is REQUIRED — there's no fallback to unsigned
# manifests; aeordb-client refuses to apply an update whose response
# doesn't carry a valid signature against the bundled trust store. If you
# genuinely want to ship without a signature (you should not), edit
# aeordb-client-lib/src/update.rs::verify_manifest_signature.

set -euo pipefail

PRIVATE_KEY=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --key|--private-key) PRIVATE_KEY="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,/^$/p' "$0" | sed 's/^# \?//'
      exit 0
      ;;
    *) echo "emit-manifest.sh: unknown argument: $1" >&2; exit 64 ;;
  esac
done
if [[ -z "$PRIVATE_KEY" ]]; then
  echo "emit-manifest.sh: --key <private-key-path> is required" >&2
  exit 64
fi
if [[ ! -f "$PRIVATE_KEY" ]]; then
  echo "emit-manifest.sh: private key not found: $PRIVATE_KEY" >&2
  exit 1
fi

cd "$(dirname "$0")/.."

# aeordb-client is a Cargo workspace; the release version lives on the
# binary crate, not the workspace root manifest.
CARGO_TOML="aeordb-client/Cargo.toml"
DOWNLOADS_DIR="../aeordb-www/downloads"
SIGNER_BIN="../../aeor-signing-tools/target/release/aeor-sign-update-manifest"

VERSION="$(grep -m1 '^version' "$CARGO_TOML" | sed -E 's/^version *= *"([^"]+)".*/\1/')"
if [[ -z "$VERSION" ]]; then
  echo "emit-manifest.sh: could not parse version from $CARGO_TOML" >&2
  exit 1
fi

if [[ ! -d "$DOWNLOADS_DIR" ]]; then
  echo "emit-manifest.sh: downloads dir not found: $DOWNLOADS_DIR" >&2
  exit 1
fi

if [[ ! -x "$SIGNER_BIN" ]]; then
  echo "emit-manifest.sh: signer binary not built — run:" >&2
  echo "  (cd ../../aeor-signing-tools && cargo build --release --bin aeor-sign-update-manifest)" >&2
  exit 1
fi

# Client release artifacts use the `aeordb-client-*` prefix to keep them
# clearly distinct from engine artifacts (`aeordb-*`) that also live in
# downloads/. The client's self-update flow downloads whatever URL is
# named here and REPLACES the running client binary with it — if these
# names ever resolved to the engine binary by mistake, every install
# that auto-updated would brick itself (a hash + signature can't catch
# that; they only attest "this file was what was signed," not "this is
# the right kind of binary"). The prefix discipline is load-bearing.
#
# macOS builds aren't in the pipeline yet — partial manifests are valid,
# the client just won't offer self-update on a missing platform. When
# macOS artifacts start landing in downloads/, add an
# `aeordb-client-macos-aarch64` block (and -x86_64 if we ship separate
# arch builds instead of a universal binary) alongside the existing two
# and re-add the macos-aarch64 / macos-x86_64 entries to the platforms
# map below.
LINUX_FILE="aeordb-client-linux-x86_64"
WINDOWS_FILE="aeordb-client-windows-x86_64.exe"
for f in "$LINUX_FILE" "$WINDOWS_FILE"; do
  if [[ ! -f "$DOWNLOADS_DIR/$f" ]]; then
    echo "emit-manifest.sh: missing artifact: $DOWNLOADS_DIR/$f" >&2
    exit 1
  fi
done

# sha256sum + stat are GNU; fall back to shasum (macOS) and stat -f.
sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'
  else                                          shasum -a 256 "$1" | awk '{print $1}'
  fi
}
filesize() {
  if stat --version >/dev/null 2>&1; then stat -c '%s' "$1"
  else                                    stat -f '%z' "$1"
  fi
}

LINUX_SHA=$(sha256 "$DOWNLOADS_DIR/$LINUX_FILE");   LINUX_SIZE=$(filesize "$DOWNLOADS_DIR/$LINUX_FILE")
WIN_SHA=$(sha256   "$DOWNLOADS_DIR/$WINDOWS_FILE"); WIN_SIZE=$(filesize "$DOWNLOADS_DIR/$WINDOWS_FILE")

RELEASED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
RELEASE_NOTES_URL="https://aeordb.com/docs/release-notes/${VERSION}.html"

cat > "$DOWNLOADS_DIR/manifest.json" <<EOF
{
  "kind": "aeordb-update-manifest",
  "manifest_version": 1,
  "version": "$VERSION",
  "released_at": "$RELEASED_AT",
  "release_notes_url": "$RELEASE_NOTES_URL",
  "platforms": {
    "linux-x86_64":   { "file": "$LINUX_FILE",   "size": $LINUX_SIZE, "sha256": "$LINUX_SHA" },
    "windows-x86_64": { "file": "$WINDOWS_FILE", "size": $WIN_SIZE,   "sha256": "$WIN_SHA" }
  }
}
EOF

# Sign it.
"$SIGNER_BIN" \
  --private-key "$PRIVATE_KEY" \
  --manifest    "$DOWNLOADS_DIR/manifest.json" \
  --out         "$DOWNLOADS_DIR/manifest.sig.json"

echo "emit-manifest.sh: wrote $DOWNLOADS_DIR/manifest.json (v$VERSION, $RELEASED_AT)"
echo "emit-manifest.sh: wrote $DOWNLOADS_DIR/manifest.sig.json"
