#!/bin/sh
# usage: ./install.sh [shot-key] [ocr-key]    e.g. ./install.sh Meta+Shift+X Meta+Shift+T
# (or set the hotkeys later in the GUI: snip-settings)
set -e
cd "$(dirname "$0")"

# ---- dependency check: collect everything missing, then fail once
missing=""
need() { # need <what> <arch pkg> <debian pkg> <fedora pkg>
    missing="$missing
  - $1    (Arch: $2 | Debian/Ubuntu: $3 | Fedora: $4)"
}
have() { command -v "$1" >/dev/null 2>&1; }

# a release tarball ships the prebuilt binary next to this script; a source checkout builds it
if [ -x ./snip ]; then BIN=./snip; else BIN=target/release/snip; fi
[ "$BIN" = ./snip ] || have cargo || need "cargo (Rust >= 1.88)" rust cargo cargo
have spectacle  || need spectacle spectacle kde-spectacle spectacle
if have tesseract; then
    tesseract --list-langs 2>/dev/null | grep -qx eng ||
        need "tesseract English data (eng)" tesseract-data-eng tesseract-ocr-eng tesseract-langpack-eng
else
    need "tesseract + English data" "tesseract tesseract-data-eng" "tesseract-ocr tesseract-ocr-eng" "tesseract tesseract-langpack-eng"
fi
have notify-send || need notify-send libnotify libnotify-bin libnotify
have fc-match    || need fc-match fontconfig fontconfig fontconfig
if ! have python3; then
    need "python3 with GTK 4 + libadwaita bindings" "python python-gobject gtk4 libadwaita" \
        "python3 python3-gi gir1.2-gtk-4.0 gir1.2-adw-1" "python3 python3-gobject gtk4 libadwaita"
elif ! python3 -c 'import gi; gi.require_version("Gtk", "4.0"); gi.require_version("Adw", "1"); from gi.repository import Gtk, Adw' >/dev/null 2>&1; then
    need "python3 GTK 4 + libadwaita bindings (gi, Gtk 4, Adw 1)" "python-gobject gtk4 libadwaita" \
        "python3-gi gir1.2-gtk-4.0 gir1.2-adw-1" "python3-gobject gtk4 libadwaita"
fi

if [ -n "$missing" ]; then
    echo "snip: missing dependencies:$missing" >&2
    exit 1
fi

case "$XDG_CURRENT_DESKTOP" in
    *KDE*) ;;
    *) echo "warning: snip targets KDE Plasma 6 (XDG_CURRENT_DESKTOP=${XDG_CURRENT_DESKTOP:-unset})" >&2 ;;
esac
if [ "$XDG_SESSION_TYPE" != wayland ] && [ -z "$WAYLAND_DISPLAY" ]; then
    echo "warning: snip needs a Wayland session (XDG_SESSION_TYPE=${XDG_SESSION_TYPE:-unset})" >&2
fi

# ---- build (source checkout only) & install into ~/.local
[ "$BIN" = ./snip ] || cargo build --release
install -Dm755 "$BIN" ~/.local/bin/snip
install -Dm644 dist/snip-shot.desktop ~/.local/share/applications/snip-shot.desktop
install -Dm644 dist/snip-ocr.desktop ~/.local/share/applications/snip-ocr.desktop
install -Dm755 dist/snip-settings.py ~/.local/bin/snip-settings
install -Dm644 dist/snip-settings.desktop ~/.local/share/applications/snip-settings.desktop
kbuildsycoca6 >/dev/null 2>&1 || true
if [ -n "$1" ]; then python3 dist/bind-shortcut.py snip-shot.desktop "$1" "Snip Screenshot"; fi
if [ -n "$2" ]; then python3 dist/bind-shortcut.py snip-ocr.desktop "$2" "Snip Text (OCR)"; fi
echo "change hotkeys with: snip-settings"
