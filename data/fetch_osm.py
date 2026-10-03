"""Download OpenStreetMap features for the area via the Overpass API.

Source: © OpenStreetMap contributors, ODbL.
The raw response is cached in raw/osm.json; pass --refresh to refetch.
"""

import json
import sys

import requests

from area import BBOX_WGS84, RAW_DIR

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


def main() -> None:
    RAW_DIR.mkdir(parents=True, exist_ok=True)
    path = RAW_DIR / "osm.json"
    if path.exists() and "--refresh" not in sys.argv:
        print(f"Using cached {path} (pass --refresh to refetch)")
        return
    data = None
    for url in OVERPASS_URLS:
        try:
            resp = requests.post(url, data={"data": build_query()}, timeout=300,
                                 headers={"User-Agent": "heathmap/0.1 (github.com/MaxWellHays/heathmap)"})
            resp.raise_for_status()
            data = resp.json()
            break
        except (requests.RequestException, ValueError) as err:
            print(f"  {url} failed: {err}")
    if data is None:
        raise SystemExit("All Overpass servers failed")
    path.write_text(json.dumps(data))
    print(f"Wrote {path} ({len(data['elements'])} elements)")


if __name__ == "__main__":
    main()
