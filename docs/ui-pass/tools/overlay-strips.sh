#!/bin/bash
# Frame strips of the overlay (the AI cursor) in WebKit under the app's CSP.
#   overlay-strips.sh <folder with overlay.html> <out dir> [dark|light]
# Writes <out>/<scene>-NN.png for every scene below (scene:wait ms:frames:interval ms[:query]).
set -e
here="$(cd "$(dirname "$0")" && pwd)"
folder="$1"; out="$2"; scheme="${3:-dark}"
[ -x /tmp/wksnap ] && [ ! "$here/wksnap.swift" -nt /tmp/wksnap ] || swiftc -O "$here/wksnap.swift" -o /tmp/wksnap
port=$((20000 + RANDOM % 20000))
python3 "$here/csp_serve.py" "$folder" "$port" "$here/../../../app/src-tauri/tauri.conf.json" >/dev/null 2>&1 &
srv=$!
trap 'kill $srv 2>/dev/null; wait $srv 2>/dev/null || true' EXIT
sleep 1
mkdir -p "$out"
SCENES=${SCENES:-"glide:100:8:130 click:350:8:70 doubleclick:350:8:70 press:350:8:80 scroll:350:7:90 drag:300:8:120 key:400:4:350 type:400:4:350 point:350:6:150 companion:300:4:600 thinking:300:4:600 caption:400:8:450 captionlow:500:2:700 captionnocursor:500:2:700 acting:300:2:500 stopped:350:4:200 hide:300:4:150 resting:2200:2:500 sleepy:600:2:700:fast=1 eyes:100:8:600:fast=1 color-pink:400:1:100:color=%23ff2d95 color-blue:400:1:100:color=%233b82f6 color-dark:400:1:100:color=%231a1a2e"}
for s in $SCENES; do
  IFS=: read -r name wait frames interval extra <<<"$s"
  scene="${name%%-*}"; [ "$name" = sleepy ] && scene=resting
  q="scene=$scene${extra:+&$extra}"; [ "$name" = eyes ] && q="scene=eyes&fast=1"
  /tmp/wksnap --scheme $scheme --frames $frames --interval $interval \
    "http://127.0.0.1:$port/overlay.html?$q" "$out/$name.png" ${OW:-520} ${OH:-300} $wait "$here/overlay-stub.js" "$here/overlay-scenes.js" | sed "s/^/$name /"
done
