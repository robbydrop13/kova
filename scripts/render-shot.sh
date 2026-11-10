#!/bin/sh
# A PNG of an HTML file written by `kova render`, with a headless Chrome:
# no window, nothing takes the screen. See docs/render.md.
#   scripts/render-shot.sh <file.html> <file.png> [width,height]
set -eu
[ $# -ge 2 ] || { echo "usage: $0 <file.html> <file.png> [width,height]" >&2; exit 2; }
html=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
png=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
size=${3:-1100,1400}
chrome=${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}
[ -x "$chrome" ] || { echo "$0: no Chrome at $chrome (set CHROME)" >&2; exit 1; }
[ -f "$html" ] || { echo "$0: $html: no such file" >&2; exit 1; }
# A file URL: spaces and the few characters a path may hold, percent-encoded.
url="file://$(printf '%s' "$html" | sed -e 's/%/%25/g' -e 's/ /%20/g' -e 's/#/%23/g' -e 's/?/%3F/g')"
rm -f "$png"
"$chrome" --headless=new --disable-gpu --hide-scrollbars --window-size="$size" --screenshot="$png" "$url" >/dev/null 2>&1
[ -s "$png" ] || { echo "$0: Chrome wrote no image" >&2; exit 1; }
echo "$png"
