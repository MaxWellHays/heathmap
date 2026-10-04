#!/usr/bin/env bash
# Builds the browser version into engine/dist/: the engine for WebGPU (gpu/) and for WebGL2
# (gl/), the shared assets, and a loader page that picks one (web/index.html).
#
#   ./build-web.sh                   # for serving dist/ at any path (relative URLs)
#   ./build-web.sh /heathmap/3d/     # public URL the site is served from
#
# Try it locally with: python3 -m http.server -d dist 8080
set -euo pipefail
cd "$(dirname "$0")"
url="${1:-./}"

rm -rf dist
trunk build web/gl.html --dist dist/gl --public-url "${url}gl/" --filehash false
trunk build web/gpu.html --dist dist/gpu --public-url "${url}gpu/" --filehash false
# The trunk pages are only build targets; the loader replaces them.
rm -f dist/gl/*.html dist/gpu/*.html
cp -r assets dist/assets
cp web/index.html dist/index.html
du -sh dist/gl/*.wasm dist/gpu/*.wasm
