#!/bin/sh
# Re-render docs/hero.gif and docs/ocr.gif from the HTML scenes in this folder.
#   ./render.sh [hero] [ocr]
# puppeteer-core is installed into a cache dir, not into the project.
# Open hero.html / ocr.html in a browser to preview the animation live.
set -e
here="$(cd "$(dirname "$0")" && pwd)"
dir="${XDG_CACHE_HOME:-$HOME/.cache}/snip-demo-render"
if [ ! -d "$dir/node_modules/puppeteer-core" ]; then
    mkdir -p "$dir"
    (cd "$dir" && npm init -y >/dev/null && npm install --no-fund --no-audit puppeteer-core >/dev/null)
fi
PUPPETEER_DIR="$dir" exec node "$here/render.mjs" "$@"
