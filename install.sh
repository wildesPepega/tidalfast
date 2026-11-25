#!/usr/bin/env bash
# Installs the latest oxytide release for the current user (no root needed).
#   curl -fsSL https://raw.githubusercontent.com/wildesPepega/oxytide/main/install.sh | bash
set -euo pipefail

REPO="wildesPepega/oxytide"
BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"

[ "$(uname -s)" = "Linux" ] || { echo "This installer is for Linux. Windows users: download the .exe from the releases page."; exit 1; }
[ "$(uname -m)" = "x86_64" ] || { echo "Only x86_64 builds are published."; exit 1; }

BASE="https://github.com/$REPO/releases/latest/download"
mkdir -p "$BIN_DIR" "$DATA_DIR/applications" "$DATA_DIR/icons/hicolor/256x256/apps"

echo "Downloading oxytide ..."
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
curl -fL --progress-bar -o "$tmp" "$BASE/oxytide-linux-x86_64"
install -m 755 "$tmp" "$BIN_DIR/oxytide"

curl -fsSL -o "$DATA_DIR/icons/hicolor/256x256/apps/oxytide.png" "https://raw.githubusercontent.com/$REPO/main/assets/icon.png"
cat > "$DATA_DIR/applications/oxytide.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Oxytide
Comment=A simple, native TIDAL client
Exec="$BIN_DIR/oxytide" %u
Icon=oxytide
Terminal=false
Categories=AudioVideo;Audio;Player;
MimeType=x-scheme-handler/spotify;x-scheme-handler/tidal;
StartupWMClass=oxytide
DESKTOP
command -v update-desktop-database >/dev/null && update-desktop-database "$DATA_DIR/applications" || true

echo "Installed to $BIN_DIR/oxytide"
case ":$PATH:" in *":$BIN_DIR:"*) ;; *) echo "Note: $BIN_DIR is not in your PATH." ;; esac
echo "Run 'oxytide' or start it from your application menu."

# the launcher entry and icon of the version that was still called "tidalfast" would show up twice
if grep -q "Name=tidalfast" "$DATA_DIR/applications/tidalfast.desktop" 2>/dev/null; then
    rm -f "$DATA_DIR/applications/tidalfast.desktop" "$DATA_DIR/icons/hicolor/256x256/apps/tidalfast.png"
fi
