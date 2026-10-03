"""Download the Environment Agency 1m LIDAR terrain model (DTM) for the area.

Source: LIDAR Composite DTM 1m, Environment Agency, Open Government Licence v3.
Fetched in 1km tiles from the EA WCS service; existing tiles are skipped.
"""

import subprocess

import requests

from area import BBOX_BNG, BUILD_DIR, RAW_DIR

WCS_URL = "https://environment.data.gov.uk/spatialdata/lidar-composite-digital-terrain-model-dtm-1m/wcs"
COVERAGE_ID = "13787b9a-26a4-4775-8523-806d13af58fc__Lidar_Composite_Elevation_DTM_1m"
TILE = 1000  # metres


def fetch_tile(e: int, n: int) -> None:
    path = RAW_DIR / "dtm" / f"{e}_{n}.tif"
    if path.exists():
        return
    params = [
        ("service", "WCS"),
        ("version", "2.0.1"),
        ("request", "GetCoverage"),
        ("CoverageId", COVERAGE_ID),
        ("format", "image/tiff"),
        ("subset", f"E({e},{e + TILE})"),
        ("subset", f"N({n},{n + TILE})"),
    ]
    resp = requests.get(WCS_URL, params=params, timeout=300)
    resp.raise_for_status()
    if not resp.headers.get("content-type", "").startswith("image/tiff"):
        raise RuntimeError(f"Unexpected response for tile {e},{n}: {resp.text[:300]}")
    path.write_bytes(resp.content)
    print(f"  fetched {path.name} ({len(resp.content) // 1024} KB)")


def main() -> None:
    (RAW_DIR / "dtm").mkdir(parents=True, exist_ok=True)
    BUILD_DIR.mkdir(parents=True, exist_ok=True)
    min_e, min_n, max_e, max_n = BBOX_BNG
    print("Fetching LIDAR DTM tiles...")
    for e in range(min_e, max_e, TILE):
        for n in range(min_n, max_n, TILE):
            fetch_tile(e, n)

    tiles = sorted(str(p) for p in (RAW_DIR / "dtm").glob("*.tif"))
    subprocess.run(["gdalbuildvrt", "-q", "-overwrite", str(BUILD_DIR / "dtm_1m.vrt"), *tiles], check=True)
    print(f"Mosaic: {BUILD_DIR / 'dtm_1m.vrt'}")


if __name__ == "__main__":
    main()
