#!/usr/bin/env bash
# Runs the whole data pipeline: downloads sources (cached in raw/) and writes
# everything the site serves into web/public/data/. Needs GDAL and uv.
set -euo pipefail
cd "$(dirname "$0")"

uv run fetch_lidar.py
uv run fetch_trees.py "$@"
uv run fetch_osm.py "$@"   # pass --refresh to re-download OSM data
uv run build_contours.py
uv run build_terrain.py
uv run build_osm.py
uv run build_places.py
uv run build_buildings.py
uv run build_trees.py
