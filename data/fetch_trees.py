"""Download tree datasets for the area (cached in raw/trees/; pass --refresh to refetch).

Sources (both Open Government Licence v3):
- Forest Research "National Trees Outside Woodland" map (V1): crown polygons of
  lone trees, tree groups and small woods, with LIDAR-derived heights.
- GLA "London Public Realm Trees": street and park trees maintained by boroughs,
  with species and size. Hampstead Heath itself (City of London) is not included.
"""

import csv
import io
import json
import sys

import requests

from area import BBOX_WGS84, RAW_DIR

TOW_URL = ("https://environment.data.gov.uk/spatialdata/national-trees-outside-woodland-map"
           "/ogc/features/v1/collections/FR_TOW_V1_London/items")
LONDON_TREES_URL = ("https://data.london.gov.uk/download/2r45m/e62a6a1f-390d-4193-ae32-3aabd9846f36"
                    "/Borough_tree_list_2025Nov.csv")
TREES_DIR = RAW_DIR / "trees"
HEADERS = {"User-Agent": "heathmap/0.1 (github.com/MaxWellHays/heathmap)"}


def fetch_tow() -> None:
    path = TREES_DIR / "tow.geojson"
    if path.exists() and "--refresh" not in sys.argv:
        print(f"Using cached {path}")
        return
    w, s, e, n = BBOX_WGS84
    features = []
    url = TOW_URL
    params: dict | None = {"f": "json", "bbox": f"{w},{s},{e},{n}", "limit": 1000}
    while url:
        resp = requests.get(url, params=params, headers=HEADERS, timeout=300)
        resp.raise_for_status()
        page = resp.json()
        features += page["features"]
        # The 'next' link already carries the query parameters.
        url = next((link["href"] for link in page.get("links", []) if link["rel"] == "next"), None)
        params = None
        print(f"  trees outside woodland: {len(features)} / {page.get('numberMatched')}")
    path.write_text(json.dumps({"type": "FeatureCollection", "features": features}))


def fetch_london_trees() -> None:
    """Keeps only the rows inside the area; the full CSV is ~200 MB."""
    path = TREES_DIR / "london_public_realm.csv"
    if path.exists() and "--refresh" not in sys.argv:
        print(f"Using cached {path}")
        return
    resp = requests.get(LONDON_TREES_URL, headers=HEADERS, timeout=900)
    resp.raise_for_status()
    w, s, e, n = BBOX_WGS84
    reader = csv.DictReader(io.StringIO(resp.content.decode("utf-8-sig")))
    with path.open("w", newline="") as out:
        writer = csv.DictWriter(out, fieldnames=reader.fieldnames)
        writer.writeheader()
        kept = 0
        for row in reader:
            try:
                lat, lon = float(row["lat"]), float(row["lon"])
            except ValueError:
                continue
            if s <= lat <= n and w <= lon <= e:
                writer.writerow(row)
                kept += 1
    print(f"  London public realm trees in area: {kept}")


def main() -> None:
    TREES_DIR.mkdir(parents=True, exist_ok=True)
    fetch_tow()
    fetch_london_trees()


if __name__ == "__main__":
    main()
