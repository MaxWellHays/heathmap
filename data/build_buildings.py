"""Extract building footprints and measure their heights from LIDAR (run fetch_lidar.py
and fetch_osm.py first).

Footprints come from OpenStreetMap (Geofabrik extract). Height is the 90th
percentile of the normalised surface (DSM − DTM) inside each footprint, which is
robust to stray LIDAR returns and to parts of the footprint at ground level
(courtyards, slight mapping misalignment). OSM's own height tags are sparse in London.

The median is stored too: on a pitched roof LIDAR heights spread evenly between the
eaves and the ridge, so p90 − p50 is large (≈ 0.4 × ridge rise for a simple gable);
on a flat roof the two are nearly equal. The engine uses this to choose roof shapes.

Outputs (build/, British National Grid):
  ndsm_1m.tif             height above ground of everything (also used by build_trees.py)
  buildings.geojson       footprints with `osm_id`, `name`, `height` (m, p90), `height_p50` (m), `ground` (m, DTM at base)
"""

import json
import subprocess

import numpy as np
import rasterio
from rasterio.features import rasterize
from shapely.geometry import mapping, shape

from area import BBOX_BNG, BUILD_DIR, RAW_DIR

HEIGHT_PERCENTILE = 90
MIN_HEIGHT = 2.5  # metres; lower "buildings" are sheds, carports or mapping errors


def build_ndsm() -> None:
    """DSM − DTM on the 1m grid, clipped at 0 (small negatives are LIDAR noise)."""
    with rasterio.open(BUILD_DIR / "dtm_1m.vrt") as dtm, rasterio.open(BUILD_DIR / "dsm_1m.vrt") as dsm:
        assert dtm.transform == dsm.transform and dtm.shape == dsm.shape, "DTM and DSM grids differ"
        ground = dtm.read(1, masked=True)
        surface = dsm.read(1, masked=True)
        ndsm = np.ma.clip(surface - ground, 0, None).filled(0).astype("float32")
        profile = dtm.profile | {"driver": "GTiff", "compress": "deflate", "predictor": 3, "tiled": True,
                                 "nodata": None}
    with rasterio.open(BUILD_DIR / "ndsm_1m.tif", "w", **profile) as out:
        out.write(ndsm, 1)


def extract_footprints() -> list[dict]:
    """Building polygons from the OSM extract, reprojected to BNG and clipped to the area."""
    out = BUILD_DIR / "buildings_osm.geojson"
    out.unlink(missing_ok=True)
    min_e, min_n, max_e, max_n = BBOX_BNG
    subprocess.run([
        "ogr2ogr", "-f", "GeoJSON", str(out), str(RAW_DIR / "greater-london.osm.pbf"), "multipolygons",
        "-where", "building IS NOT NULL AND building <> 'no'",  # building=no marks non-buildings (e.g. a barrow)
        "-t_srs", "EPSG:27700",
        "-spat", str(min_e), str(min_n), str(max_e), str(max_n), "-spat_srs", "EPSG:27700",
        "-clipdst", str(min_e), str(min_n), str(max_e), str(max_n),
        "-select", "osm_id,osm_way_id,name,building",
    ], check=True, stderr=subprocess.DEVNULL)
    return json.loads(out.read_text())["features"]


def main() -> None:
    build_ndsm()
    features = extract_footprints()
    geoms = [shape(f["geometry"]) for f in features]

    with rasterio.open(BUILD_DIR / "ndsm_1m.tif") as src:
        ndsm = src.read(1)
        transform = src.transform
    with rasterio.open(BUILD_DIR / "dtm_1m.vrt") as dtm:
        ground = dtm.read(1, masked=True).filled(np.nan)

    # Label each 1m cell with the index (+1) of the building covering it, then group cells by label.
    labels = rasterize(((g, i + 1) for i, g in enumerate(geoms)), out_shape=ndsm.shape,
                       transform=transform, dtype="int32")
    flat = labels.ravel()
    order = np.argsort(flat, kind="stable")
    sorted_labels = flat[order]
    starts = np.searchsorted(sorted_labels, np.arange(1, len(geoms) + 2))

    out_features = []
    for i, (f, g) in enumerate(zip(features, geoms)):
        cells = order[starts[i]:starts[i + 1]]
        if len(cells) == 0:  # footprint smaller than one cell
            continue
        values = ndsm.ravel()[cells]
        height, median = (float(v) for v in np.percentile(values, [HEIGHT_PERCENTILE, 50]))
        if height < MIN_HEIGHT:
            continue
        p = f["properties"]
        osm_id = f"way/{p['osm_way_id']}" if p.get("osm_way_id") else f"relation/{p['osm_id']}"
        out_features.append({
            "type": "Feature",
            "properties": {"osm_id": osm_id, "name": p.get("name"), "building": p.get("building"),
                           "height": round(height, 1), "height_p50": round(median, 1),
                           "ground": round(float(np.nanmin(ground.ravel()[cells])), 1)},
            "geometry": mapping(g),
        })

    path = BUILD_DIR / "buildings.geojson"
    path.write_text(json.dumps({"type": "FeatureCollection", "features": out_features}))
    heights = np.array([f["properties"]["height"] for f in out_features])
    print(f"Wrote {path}: {len(out_features)} buildings (of {len(features)} footprints), "
          f"height median {np.median(heights):.1f} m, max {heights.max():.1f} m")


if __name__ == "__main__":
    main()
