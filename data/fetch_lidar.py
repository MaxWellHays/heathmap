"""Download Environment Agency 1m LIDAR elevation models for the area.

Sources (Environment Agency, Open Government Licence v3), fetched in 1km tiles
from the EA WCS services; existing tiles are skipped:
- DTM (LIDAR Composite DTM 1m): bare-earth terrain, buildings and vegetation removed.
- DSM (LIDAR Composite First Return DSM 1m): surface including buildings and tree canopy.
  DSM minus DTM gives the height of everything standing on the ground.
"""

import subprocess

import requests

from area import BBOX_BNG, BUILD_DIR, RAW_DIR

MODELS = {
    "dtm": (
        "https://environment.data.gov.uk/spatialdata/lidar-composite-digital-terrain-model-dtm-1m/wcs",
        "13787b9a-26a4-4775-8523-806d13af58fc__Lidar_Composite_Elevation_DTM_1m",
    ),
    "dsm": (
        "https://environment.data.gov.uk/spatialdata/lidar-composite-digital-surface-model-first-return-dsm-1m/wcs",
        "df4e3ec3-315e-48aa-aaaf-b5ae74d7b2bb__Lidar_Composite_Elevation_FZ_DSM_1m",
    ),
}
TILE = 1000  # metres


def fetch_tile(model: str, e: int, n: int) -> None:
    path = RAW_DIR / model / f"{e}_{n}.tif"
    if path.exists():
        return
    url, coverage = MODELS[model]
    params = [
        ("service", "WCS"),
        ("version", "2.0.1"),
        ("request", "GetCoverage"),
        ("CoverageId", coverage),
        ("format", "image/tiff"),
        ("subset", f"E({e},{e + TILE})"),
        ("subset", f"N({n},{n + TILE})"),
    ]
    resp = requests.get(url, params=params, timeout=300)
    resp.raise_for_status()
    if not resp.headers.get("content-type", "").startswith("image/tiff"):
        raise RuntimeError(f"Unexpected response for {model} tile {e},{n}: {resp.text[:300]}")
    path.write_bytes(resp.content)
    print(f"  fetched {model}/{path.name} ({len(resp.content) // 1024} KB)")


def main() -> None:
    BUILD_DIR.mkdir(parents=True, exist_ok=True)
    min_e, min_n, max_e, max_n = BBOX_BNG
    for model in MODELS:
        (RAW_DIR / model).mkdir(parents=True, exist_ok=True)
        print(f"Fetching LIDAR {model.upper()} tiles...")
        for e in range(min_e, max_e, TILE):
            for n in range(min_n, max_n, TILE):
                fetch_tile(model, e, n)
        tiles = sorted(str(p) for p in (RAW_DIR / model).glob("*.tif"))
        vrt = BUILD_DIR / f"{model}_1m.vrt"
        subprocess.run(["gdalbuildvrt", "-q", "-overwrite", str(vrt), *tiles], check=True)
        print(f"Mosaic: {vrt}")


if __name__ == "__main__":
    main()
