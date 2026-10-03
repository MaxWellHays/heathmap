"""Build Terrarium-encoded elevation tiles from the LIDAR DTM (run fetch_lidar.py first).

MapLibre reads these through a `raster-dem` source for the 3D terrain, the
elevation colour fill (color-relief) and hillshading.

Terrarium encoding: elevation = R * 256 + G + B / 256 - 32768 (metres).
"""

import json
import shutil
import subprocess

import mercantile
import numpy as np
import rasterio
import rasterio.warp
from PIL import Image
from rasterio.enums import Resampling
from rasterio.fill import fillnodata
from rasterio.windows import from_bounds
from scipy.ndimage import distance_transform_edt

from area import BUILD_DIR, OUT_DIR

MIN_ZOOM = 12  # keep web config `map.minZoom` at or above this
MAX_ZOOM = 15  # ~3 m per pixel at this latitude; MapLibre overzooms beyond it
TILE_SIZE = 256
QUANTUM = 0.1  # metres; rounding elevations makes the PNGs compress far better

MERCATOR_DTM = BUILD_DIR / "dtm_3857.tif"
TILES_DIR = OUT_DIR / "terrain"


def build_mercator_dtm() -> None:
    """Reproject the 1m DTM to Web Mercator at the max zoom's pixel size, filling gaps."""
    z_res = 2 * 20037508.342789244 / (TILE_SIZE * 2**MAX_ZOOM)
    tmp = BUILD_DIR / "dtm_3857_raw.tif"
    subprocess.run(["gdalwarp", "-q", "-overwrite", "-t_srs", "EPSG:3857", "-r", "average",
                    "-tr", str(z_res), str(z_res), "-dstnodata", "-9999",
                    str(BUILD_DIR / "dtm_1m.vrt"), str(tmp)], check=True)
    with rasterio.open(tmp) as src:
        data = src.read(1)
        profile = src.profile
    # Fill LIDAR gaps (water bodies, voids) from surrounding cells.
    data = fillnodata(data, mask=(data != -9999), max_search_distance=200)
    with rasterio.open(MERCATOR_DTM, "w", **profile) as dst:
        dst.write(data, 1)
    tmp.unlink()


def fill_from_nearest(ele: np.ma.MaskedArray) -> np.ndarray:
    """Give cells outside the data the value of the nearest data cell.

    Tiles on the edge of the area extend past the LIDAR coverage; edge-extending
    avoids the terrain dropping off a cliff to sea level there.
    """
    if not np.ma.is_masked(ele):
        return ele.data
    _, (rows, cols) = distance_transform_edt(ele.mask, return_indices=True)
    return ele.data[rows, cols]


def encode_terrarium(ele: np.ndarray) -> Image.Image:
    v = np.round(ele / QUANTUM) * QUANTUM + 32768
    r = np.floor(v / 256)
    g = np.floor(v - r * 256)
    b = np.round((v - r * 256 - g) * 256)
    return Image.fromarray(np.dstack([r, g, b]).clip(0, 255).astype(np.uint8), "RGB")


def main() -> None:
    build_mercator_dtm()
    shutil.rmtree(TILES_DIR, ignore_errors=True)

    with rasterio.open(MERCATOR_DTM) as src:
        nodata = src.nodata
        west, south, east, north = rasterio.warp.transform_bounds(src.crs, "EPSG:4326", *src.bounds)
        all_data = src.read(1, masked=True)
        min_ele, max_ele = float(all_data.min()), float(all_data.max())
        count = 0
        for tile in mercantile.tiles(west, south, east, north, range(MIN_ZOOM, MAX_ZOOM + 1)):
            b = mercantile.xy_bounds(tile)
            window = from_bounds(b.left, b.bottom, b.right, b.top, src.transform)
            ele = src.read(1, window=window, out_shape=(TILE_SIZE, TILE_SIZE), boundless=True,
                           masked=True, fill_value=nodata, resampling=Resampling.average)
            ele = fill_from_nearest(ele)
            path = TILES_DIR / str(tile.z) / str(tile.x) / f"{tile.y}.png"
            path.parent.mkdir(parents=True, exist_ok=True)
            encode_terrarium(ele).save(path, optimize=True)
            count += 1

    meta = {
        "minzoom": MIN_ZOOM,
        "maxzoom": MAX_ZOOM,
        "tileSize": TILE_SIZE,
        "encoding": "terrarium",
        "bounds": [round(west, 5), round(south, 5), round(east, 5), round(north, 5)],
        "minEle": round(min_ele, 1),
        "maxEle": round(max_ele, 1),
    }
    (OUT_DIR / "terrain.meta.json").write_text(json.dumps(meta, indent=2) + "\n")
    size = sum(p.stat().st_size for p in TILES_DIR.rglob("*.png"))
    print(f"Wrote {count} tiles ({size // 1024} KB) to {TILES_DIR}: {meta}")


if __name__ == "__main__":
    main()
