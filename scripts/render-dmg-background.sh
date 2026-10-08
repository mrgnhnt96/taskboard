#!/bin/bash
# Renders packaging/assets/dmg/background.svg to background.tiff, the DMG
# window's background, at 1x and 2x so it's sharp on Retina screens.
#
# Run it after changing background.svg and commit the TIFF; releases use
# the committed file. Needs ImageMagick (brew install imagemagick).
set -euo pipefail
cd "$(dirname "$0")/.."

dir=packaging/assets/dmg
font=crates/taskboard-app/assets/fonts/AtkinsonHyperlegibleNext-Bold.ttf
command -v magick >/dev/null || { echo "Needs ImageMagick: brew install imagemagick" >&2; exit 1; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# At 96 dpi per scale, one SVG unit is one point; 13.5 pt there is 18 px at 1x.
for scale in 1 2; do
  magick -density $((96 * scale)) "MSVG:$dir/background.svg" -background none -flatten \
    -font "$font" -pointsize 13.5 -fill "#1f2a4d" -gravity north \
    -annotate +0+$((332 * scale)) "Drag Taskboard into Applications" \
    -units PixelsPerInch -density $((72 * scale)) "$tmp/background@${scale}x.png"
done
sips -g pixelWidth -g pixelHeight "$tmp/background@1x.png" "$tmp/background@2x.png" | grep pixel
# One TIFF holding both sizes; Finder picks the one for the screen.
tiffutil -cathidpicheck "$tmp/background@1x.png" "$tmp/background@2x.png" -out "$dir/background.tiff"
echo "Wrote $dir/background.tiff"
