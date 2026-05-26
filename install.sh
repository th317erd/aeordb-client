#!/usr/bin/env bash
#
# install.sh — Build and install the `aeordb-client` binary.
#
# Default install location: ~/.local/bin (XDG user-bin convention).
# Use --system to install to /usr/local/bin (requires sudo).
#
# Side effects:
#   - Builds --release with -j 2 if no release binary exists. The -j 2
#     cap is mandatory — unrestricted parallel cargo builds OOM-kill
#     other processes on this user's machine (see CLAUDE.md).
#   - Installs a .desktop launcher to ~/.local/share/applications/.
#   - Installs the app icon to ~/.local/share/icons/hicolor/512x512/apps/.
#   - If an autostart entry already exists at
#     ~/.config/autostart/aeordb-client.desktop, rewrites it to point at
#     the freshly-installed binary. (When the running daemon next starts,
#     tauri-plugin-autostart would do this itself; the rewrite-on-install
#     covers the case where the user installs but doesn't restart the
#     daemon before the next reboot.)
#
# Usage:
#   ./install.sh                # user install to ~/.local/bin
#   ./install.sh --system       # /usr/local/bin (requires sudo)
#   ./install.sh --dev          # symlink to target/debug for dev-watch
#   ./install.sh -h | --help    # this help

set -euo pipefail

repo_root="$(cd "$(dirname "$0")" && pwd)"

mode="user"
dev_mode=0
for arg in "$@"; do
  case "$arg" in
    --system) mode="system" ;;
    --dev)    dev_mode=1 ;;
    -h|--help)
      sed -n '3,24p' "$0"
      exit 0
      ;;
    *)
      echo "unknown argument: $arg" >&2
      exit 2
      ;;
  esac
done

# ---------------------------------------------------------------------------
# Platform gate
# ---------------------------------------------------------------------------

case "$(uname -s)" in
  Linux|Darwin) ;;
  *)
    echo "install.sh only supports Linux/macOS. Use the platform's native installer on $(uname -s)." >&2
    exit 2
    ;;
esac

# ---------------------------------------------------------------------------
# Resolve install paths
# ---------------------------------------------------------------------------

if [[ "$mode" == "system" ]]; then
  install_bin_dir="/usr/local/bin"
  install_sudo="sudo"
else
  install_bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
  install_sudo=""
fi
install_bin="$install_bin_dir/aeordb-client"

desktop_dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
desktop_file="$desktop_dir/aeordb-client.desktop"
icon_dir="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/512x512/apps"
icon_file="$icon_dir/aeordb-client.png"

autostart_file="${XDG_CONFIG_HOME:-$HOME/.config}/autostart/aeordb-client.desktop"

# ---------------------------------------------------------------------------
# Build the binary
# ---------------------------------------------------------------------------

if [[ "$dev_mode" == "1" ]]; then
  source_bin="$repo_root/target/debug/aeordb-client"
  if [[ ! -x "$source_bin" ]]; then
    echo "[install] building debug binary (-j 2)..."
    ( cd "$repo_root" && cargo build -j 2 --bin aeordb-client )
  fi
else
  source_bin="$repo_root/target/release/aeordb-client"
  if [[ ! -x "$source_bin" ]]; then
    echo "[install] building release binary (-j 2)..."
    # -j 2 cap is mandatory: unrestricted parallel rustc OOM-kills other
    # processes on this user's box. Don't drop it.
    ( cd "$repo_root" && cargo build -j 2 --release --bin aeordb-client )
  fi
fi
[[ -x "$source_bin" ]] || {
  echo "[install] source binary missing after build: $source_bin" >&2
  exit 1
}

# ---------------------------------------------------------------------------
# Install binary + icon + launcher .desktop
# ---------------------------------------------------------------------------

if [[ "$dev_mode" == "1" ]]; then
  if [[ "$mode" == "system" ]]; then
    echo "[install] --dev with --system isn't supported (a symlink in /usr/local/bin into a user repo is fragile)." >&2
    exit 2
  fi
  mkdir -p "$install_bin_dir"
  echo "[install] symlinking $install_bin -> $source_bin"
  ln -sfn "$source_bin" "$install_bin"
else
  echo "[install] installing $install_bin"
  $install_sudo install -D -m 755 "$source_bin" "$install_bin"
fi

# Icon (best-effort — failure here doesn't break the install).
if [[ -f "$repo_root/aeordb-client/icons/icon.png" ]]; then
  mkdir -p "$icon_dir"
  install -m 644 "$repo_root/aeordb-client/icons/icon.png" "$icon_file"
fi

# Launcher entry (apps menu). No --start-minimized here — clicking the
# launcher from the menu should pop the window, like any other app.
mkdir -p "$desktop_dir"
cat > "$desktop_file" <<DESKTOP
[Desktop Entry]
Type=Application
Version=1.0
Name=AeorDB Client
Comment=Sync-first desktop client for AeorDB
Exec=$install_bin
Icon=aeordb-client
Terminal=false
Categories=Utility;Network;FileTransfer;
StartupNotify=false
DESKTOP

# Refresh the desktop database so launchers find the new entry without a
# logout/login cycle. Non-fatal if the helper isn't installed.
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$desktop_dir" >/dev/null 2>&1 || true
fi

# ---------------------------------------------------------------------------
# Rewrite the autostart entry if one exists
# ---------------------------------------------------------------------------
#
# The plugin-managed autostart entry uses whatever `current_exe()`
# resolves to at write-time. If the user toggled the checkbox while
# running the dev build, the entry points at target/debug/aeordb-client
# — which won't survive a `cargo clean` or a fresh checkout. Rewrite it
# to the installed binary so autostart survives those, AND so the
# launched daemon at boot is the release build (faster, smaller).
#
# When the installed daemon next starts, its boot-time reconciliation
# will rewrite this same file with `current_exe()` again — but since
# that will now BE the installed path, the file stays correct.

if [[ -f "$autostart_file" ]]; then
  echo "[install] rewriting autostart entry to use $install_bin"
  cat > "$autostart_file" <<AUTOSTART
[Desktop Entry]
Type=Application
Version=1.0
Name=aeordb-client
Comment=aeordb-clientstartup script
Exec=$install_bin start --start-minimized
StartupNotify=false
Terminal=false
AUTOSTART
else
  echo "[install] autostart entry not present (toggle 'Start when system starts' in Settings to enable)"
fi

# ---------------------------------------------------------------------------
# PATH sanity check
# ---------------------------------------------------------------------------

if [[ "$mode" == "user" ]]; then
  case ":$PATH:" in
    *":$install_bin_dir:"*) ;;
    *)
      echo
      echo "[warn] $install_bin_dir is not on your PATH."
      echo "       Add this to your shell rc (e.g. ~/.bashrc or ~/.zshrc):"
      echo "         export PATH=\"\$HOME/.local/bin:\$PATH\""
      ;;
  esac
fi

echo
echo "[install] aeordb-client installed to $install_bin"
echo "[install] launcher: $desktop_file"
if [[ -f "$autostart_file" ]]; then
  echo "[install] autostart: $autostart_file"
fi
echo
echo "Next steps:"
echo "  1. Stop the dev daemon if one is running (./scripts/stop-client.sh)"
echo "  2. Run the installed binary once so the autostart entry is freshly"
echo "     reconciled against the new exe path:"
echo "         $install_bin start --start-minimized &"
echo "     (close the window — it'll hide to tray, daemon stays up)"
