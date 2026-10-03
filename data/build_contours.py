"""Generate contour lines from the LIDAR DTM mosaic (run fetch_dtm.py first).

The 1m DTM is resampled to a coarser grid with averaging, which smooths away
LIDAR noise (kerbs, ditches, tree pits) that would otherwise make jagged
contours. Contours are generated at a fine base interval; the site decides
which ones to draw (minor/major intervals are configurable there).
"""

import json
import subprocess

from area import BBOX_WGS84, BUILD_DIR, OUT_DIR

RESOLUTION = 3  # metres per pixel after smoothing
INTERVAL = 1  # metres between generated contours
SIMPLIFY = 0.5  # metres, Douglas-Peucker tolerance (applied in BNG)


def run(*args: str) -> None:
    subprocess.run(args, check=True)


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    src = BUILD_DIR / "dtm_1m.vrt"
    smoothed = BUILD_DIR / f"dtm_{RESOLUTION}m.tif"
    contours_bng = BUILD_DIR / "contours_bng.gpkg"
    out = OUT_DIR / "contours.geojson"

    run("gdalwarp", "-q", "-overwrite", "-r", "average", "-tr", str(RESOLUTION), str(RESOLUTION),
        str(src), str(smoothed))
    contours_bng.unlink(missing_ok=True)
    run("gdal_contour", "-q", "-a", "ele", "-i", str(INTERVAL), str(smoothed), str(contours_bng))

    out.unlink(missing_ok=True)
    w, s, e, n = BBOX_WGS84
    run("ogr2ogr", "-f", "GeoJSON", str(out), str(contours_bng),
        "-simplify", str(SIMPLIFY),
        "-t_srs", "EPSG:4326",
        "-clipdst", str(w), str(s), str(e), str(n),
        "-select", "ele",
        "-lco", "RFC7946=YES", "-lco", "COORDINATE_PRECISION=6")

    # Record the elevation range so the site can scale its colour gradient.
    eles = [f["properties"]["ele"] for f in json.loads(out.read_text())["features"]]
    meta = {"interval": INTERVAL, "minEle": min(eles), "maxEle": max(eles)}
    (OUT_DIR / "contours.meta.json").write_text(json.dumps(meta, indent=2) + "\n")
    print(f"Wrote {out} ({out.stat().st_size // 1024} KB, {len(eles)} lines, {meta})")


if __name__ == "__main__":
    main()
