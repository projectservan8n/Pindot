#!/bin/bash
# Screenshots of the main window screens with Tauri stubbed (no app is launched).
#   shots.sh <out dir> <name=hash?query> ...
# Builds the frontend, serves a copy of dist with tauri-mock.js injected, and
# takes 1x screenshots at the real window size (820x640) in headless Chrome.
set -e
here="$(cd "$(dirname "$0")" && pwd)"
repo="$here/../../.."
out="$1"; shift
work="$(mktemp -d)"
(cd "$repo/app" && npx vite build --outDir "$work/dist" --emptyOutDir >/dev/null 2>&1)
# LIGHT=1: the light theme (headless Chrome is dark), by switching off the dark media query.
[ -n "$LIGHT" ] && sed -i '' 's/prefers-color-scheme:dark/prefers-color-scheme:none/g; s/prefers-color-scheme: dark/prefers-color-scheme: none/g' "$work"/dist/assets/*.css
cp "$here/tauri-mock.js" "$work/dist/"
cp "$here"/mock-*.js "$work/dist/" 2>/dev/null || true
extra=""
for f in "$work"/dist/mock-*.js; do [ -e "$f" ] && extra="$extra<script src=\"/$(basename "$f")\"></script>"; done
sed -i '' "s#<script type=\"module\"#$extra<script src=\"/tauri-mock.js\"></script><script type=\"module\"#" "$work/dist/index.html"
# Page-specific mocks must load before tauri-mock.js.
port=$((20000 + RANDOM % 20000))
(cd "$work/dist" && python3 -m http.server "$port" --bind 127.0.0.1 >/dev/null 2>&1) &
srv=$!
trap 'kill $srv 2>/dev/null; rm -rf "$work"' EXIT
sleep 1
mkdir -p "$out"
chrome="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
for spec in "$@"; do
  name="${spec%%=*}"; url="${spec#*=}"
  prof="$(mktemp -d)"
  "$chrome" --headless=new --disable-gpu --hide-scrollbars --user-data-dir="$prof" --force-device-scale-factor=1 $CHROME_EXTRA \
    --window-size=${W:-820},${H:-640} --virtual-time-budget=4000 --screenshot="$out/$name.png" \
    "http://127.0.0.1:$port/index.html$url" >/dev/null 2>&1 || true
  rm -rf "$prof"
  echo "$name"
done
