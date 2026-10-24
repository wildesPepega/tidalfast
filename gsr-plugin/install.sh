#!/bin/sh
# Builds the plugin, installs it for GPU Screen Recorder and puts a small
# wrapper called `gpu-screen-recorder` into ~/.local/bin that loads it
# (so gsr-ui and the scripts pick it up when ~/.local/bin comes first in PATH).
# Needs gcc and the gpu-screen-recorder headers (/usr/include/gsr/plugin.h).
# Undo with: ./install.sh --remove
set -e
cd "$(dirname "$0")"
dest="${XDG_DATA_HOME:-$HOME/.local/share}/tidalfast/gsr"
wrapper="$HOME/.local/bin/gpu-screen-recorder"

if [ "$1" = "--remove" ]; then
    if grep -q tidalfast_overlay "$wrapper" 2>/dev/null; then rm -f "$wrapper"; fi
    rm -rf "$dest"
    echo "removed"
    exit 0
fi

real=$(PATH=$(echo "$PATH" | sed "s|$HOME/.local/bin:\?||g") command -v gpu-screen-recorder) || {
    echo "gpu-screen-recorder not found" >&2; exit 1; }
make -s
mkdir -p "$dest" "$HOME/.local/bin"
install -m 755 tidalfast_overlay.so "$dest/tidalfast_overlay.so"
cat > "$wrapper" <<WRAP
#!/bin/sh
# written by tidalfast's gsr-plugin/install.sh: loads the now-playing overlay
case "\$*" in
  *--list*|*--version*|*-h*|*--help*) exec "$real" "\$@" ;;
esac
exec "$real" "\$@" -p "$dest/tidalfast_overlay.so"
WRAP
chmod 755 "$wrapper"
echo "installed: $dest/tidalfast_overlay.so"
echo "wrapper:   $wrapper -> $real"
echo "Switch the overlay on in tidalfast: Settings -> Overlay."
