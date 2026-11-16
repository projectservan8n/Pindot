#!/bin/bash
# Screenshots of the main window screens with Tauri stubbed (no app is launched).
#   shots.sh <out dir> <name=hash?query> ...
# Builds the frontend, serves a copy of dist with tauri-mock.js injected and
# takes 1x screenshots at the real window size (820x640).
#
# On a Mac it renders in the system WebKit (the engine the real app runs in)
# and serves the pages under the app's Content-Security-Policy with a nonce
# (csp_serve.py), so a style="..." attribute is dropped here exactly as it is
# in the app. Elsewhere (or with ENGINE=chrome) it falls back to headless
# Chrome with NO security policy, which hides that whole class of bug.
#   LIGHT=1   light theme (default is dark)
#   W, H      window size
#   ENGINE    webkit | chrome (default webkit on macOS)
set -e
here="$(cd "$(dirname "$0")" && pwd)"
repo="$here/../../.."
out="$1"; shift
engine="${ENGINE:-}"
if [ -z "$engine" ]; then [ "$(uname)" = Darwin ] && [ -n "$(command -v swiftc)" ] && engine=webkit || engine=chrome; fi
if [ "$engine" = webkit ]; then
  wksnap="${WKSNAP:-/tmp/wksnap}"
  if [ ! -x "$wksnap" ] || [ "$here/wksnap.swift" -nt "$wksnap" ]; then swiftc -O "$here/wksnap.swift" -o "$wksnap"; fi
fi
work="$(mktemp -d)"
srv=""
trap 'if [ -n "$srv" ]; then kill $srv 2>/dev/null; wait $srv 2>/dev/null || true; fi; rm -rf "$work"' EXIT
(cd "$repo/app" && npx vite build --outDir "$work/dist" --emptyOutDir >/dev/null 2>&1)
if [ "$engine" = chrome ] && [ -n "$LIGHT" ]; then
  # headless Chrome is dark: switch the dark media query off
  sed -i '' 's/prefers-color-scheme:dark/prefers-color-scheme:none/g; s/prefers-color-scheme: dark/prefers-color-scheme: none/g; s/color-scheme:light dark/color-scheme:light/g' "$work"/dist/assets/*.css
fi
cp "$here/tauri-mock.js" "$work/dist/"
cp "$here"/mock-*.js "$work/dist/" 2>/dev/null || true
extra=""
for f in "$work"/dist/mock-*.js; do [ -e "$f" ] && extra="$extra<script src=\"/$(basename "$f")\"></script>"; done
# Page-specific mocks must load before tauri-mock.js.
sed -i '' "s#<script type=\"module\"#$extra<script src=\"/tauri-mock.js\"></script><script type=\"module\"#" "$work/dist/index.html"
port=$((20000 + RANDOM % 20000))
# Started directly (not in a subshell) so $! is the server itself and the trap stops it.
if [ "$engine" = webkit ]; then
  python3 "$here/csp_serve.py" "$work/dist" "$port" "$repo/app/src-tauri/tauri.conf.json" >/dev/null 2>&1 &
else
  python3 -m http.server "$port" --bind 127.0.0.1 --directory "$work/dist" >/dev/null 2>&1 &
fi
srv=$!
sleep 1
mkdir -p "$out"
chrome="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
scheme=dark; [ -n "$LIGHT" ] && scheme=light
for spec in "$@"; do
  name="${spec%%=*}"; url="${spec#*=}"
  if [ "$engine" = webkit ]; then
    "$wksnap" --scheme $scheme "http://127.0.0.1:$port/index.html$url" "$out/$name.png" ${W:-820} ${H:-640} 1200 >/dev/null
  else
    prof="$(mktemp -d)"
    "$chrome" --headless=new --disable-gpu --hide-scrollbars --user-data-dir="$prof" --force-device-scale-factor=1 $CHROME_EXTRA \
      --window-size=${W:-820},${H:-640} --virtual-time-budget=4000 --screenshot="$out/$name.png" \
      "http://127.0.0.1:$port/index.html$url" >/dev/null 2>&1 || true
    rm -rf "$prof"
  fi
  echo "$name"
done
