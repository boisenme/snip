#!/bin/sh
# Removes the snip hotkeys and everything install.sh put into ~/.local.
cd "$(dirname "$0")"

for d in snip-shot.desktop snip-ocr.desktop; do
    python3 dist/bind-shortcut.py --unbind "$d" ||
        echo "warning: could not remove the $d shortcut (System Settings > Shortcuts)" >&2
done

rm -f ~/.local/bin/snip \
      ~/.local/bin/snip-settings \
      ~/.local/share/applications/snip-shot.desktop \
      ~/.local/share/applications/snip-ocr.desktop \
      ~/.local/share/applications/snip-settings.desktop
if command -v kbuildsycoca6 >/dev/null 2>&1; then kbuildsycoca6 >/dev/null 2>&1 || true; fi
echo "snip uninstalled"
