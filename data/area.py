"""Shared definition of the mapped area and output locations."""

from pathlib import Path

ROOT = Path(__file__).resolve().parent
RAW_DIR = ROOT / "raw"  # downloaded source data (gitignored)
BUILD_DIR = ROOT / "build"  # intermediate products (gitignored)
OUT_DIR = ROOT.parent / "web" / "public" / "data"  # files served by the site

# Hampstead Heath with Golders Hill Park, Kenwood and the surrounding streets.
# OSM features and contours are clipped to this box.
BBOX_WGS84 = (-0.200, 51.548, -0.140, 51.580)  # west, south, east, north

# Elevation data extent in British National Grid (EPSG:27700), whole km.
# Wider than BBOX_WGS84 so the 3D terrain covers everywhere the map can pan to
# (keep web config `map.maxBounds` inside roughly -0.225, 51.535, -0.115, 51.592).
BBOX_BNG = (523000, 183000, 531000, 190000)  # min E, min N, max E, max N
