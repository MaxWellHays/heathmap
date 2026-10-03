"""Download OpenStreetMap features for the area via the Overpass API.

Source: © OpenStreetMap contributors, ODbL.
- raw/osm.json: Heath features via the Overpass API
- raw/greater-london.osm.pbf: Geofabrik extract, for buildings over the whole elevation extent
Both are cached; pass --refresh to refetch.
"""

import json
import sys

import requests

from area import BBOX_WGS84, RAW_DIR

EXTRACT_URL = "https://download.geofabrik.de/europe/united-kingdom/england/greater-london-latest.osm.pbf"
HEADERS = {"User-Agent": "heathmap/0.1 (github.com/MaxWellHays/heathmap)"}

# Tried in order; the public servers are often overloaded.
OVERPASS_URLS = [
    "https://overpass-api.de/api/interpreter",
    "https://overpass.private.coffee/api/interpreter",
    "https://maps.mail.ru/osm/tools/overpass/api/interpreter",
]


def build_query() -> str:
    w, s, e, n = BBOX_WGS84
    bbox = f"{s},{w},{n},{e}"
    return f"""
[out:json][timeout:180][bbox:{bbox}];
(
  way[highway~"^(footway|path|track|bridleway|cycleway|steps|pedestrian)$"];
  nwr[natural~"^(wood|scrub|heath|grassland|water|wetland|tree_row|peak|hill)$"];
  nwr[landuse~"^(forest|grass|meadow|recreation_ground|allotments|cemetery)$"];
  nwr[leisure~"^(park|nature_reserve|garden|pitch|swimming_area|playground|common)$"];
  nwr[water];
  nwr[tourism~"^(attraction|museum|viewpoint|artwork|gallery)$"];
  nwr[historic];
  nwr[amenity~"^(cafe|pub|drinking_water|toilets)$"];
  node[place~"^(locality|hamlet|neighbourhood)$"];
);
out geom;
"""


def fetch_extract() -> None:
    """Geofabrik's daily Greater London extract, for data too big for Overpass (buildings)."""
    path = RAW_DIR / "greater-london.osm.pbf"
    if path.exists() and "--refresh" not in sys.argv:
        print(f"Using cached {path} (pass --refresh to refetch)")
        return
    with requests.get(EXTRACT_URL, stream=True, timeout=600, headers=HEADERS) as resp:
        resp.raise_for_status()
        with path.open("wb") as f:
            for chunk in resp.iter_content(chunk_size=1 << 20):
                f.write(chunk)
    print(f"Wrote {path} ({path.stat().st_size >> 20} MB)")


def overpass(query: str) -> dict:
    for url in OVERPASS_URLS:
        try:
            resp = requests.post(url, data={"data": query}, timeout=400, headers=HEADERS)
            resp.raise_for_status()
            return resp.json()
        except (requests.RequestException, ValueError) as err:
            print(f"  {url} failed: {err}")
    raise SystemExit("All Overpass servers failed")


def fetch(name: str, query: str) -> None:
    path = RAW_DIR / name
    if path.exists() and "--refresh" not in sys.argv:
        print(f"Using cached {path} (pass --refresh to refetch)")
        return
    data = overpass(query)
    path.write_text(json.dumps(data))
    print(f"Wrote {path} ({len(data['elements'])} elements)")


def main() -> None:
    RAW_DIR.mkdir(parents=True, exist_ok=True)
    fetch("osm.json", build_query())
    fetch_extract()


if __name__ == "__main__":
    main()
