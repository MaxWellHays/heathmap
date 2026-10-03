"""Split the raw OSM download into the GeoJSON layers used by the site.

Outputs (in web/public/data/):
  paths.geojson      footpaths/tracks with `highway` and `surface` (raw OSM values;
                     the site groups surfaces into materials via its config)
  landcover.geojson  vegetation and land-use polygons with a `class`
  water.geojson      ponds, lakes and streams
  parks.geojson      park and nature reserve boundaries
  amenities.geojson  cafés, toilets and drinking water inside the parks
"""

import json

import osm2geojson
from shapely.geometry import box, mapping, shape
from shapely.ops import unary_union

from area import BBOX_WGS84, OUT_DIR, RAW_DIR

PATH_HIGHWAYS = {"footway", "path", "track", "bridleway", "cycleway", "steps", "pedestrian"}
# Footways mapped as part of streets, not walking routes in their own right.
STREET_FOOTWAYS = {"sidewalk", "crossing", "traffic_island", "access_aisle"}

LANDCOVER_RULES = [  # (tag, values, class) — first match wins
    ("natural", {"wood"}, "wood"),
    ("landuse", {"forest"}, "wood"),
    ("natural", {"scrub"}, "scrub"),
    ("natural", {"heath"}, "heath"),
    ("natural", {"wetland"}, "wetland"),
    ("landuse", {"meadow"}, "meadow"),
    ("natural", {"grassland"}, "meadow"),
    ("landuse", {"grass", "recreation_ground", "village_green"}, "grass"),
    ("leisure", {"pitch"}, "pitch"),
    ("leisure", {"garden"}, "garden"),
    ("leisure", {"playground"}, "playground"),
    ("landuse", {"cemetery"}, "cemetery"),
    ("landuse", {"allotments"}, "allotments"),
]

AMENITIES = {"cafe", "toilets", "drinking_water"}
PRECISION = 6


def round_coords(obj):
    if isinstance(obj, float):
        return round(obj, PRECISION)
    if isinstance(obj, (list, tuple)):
        return [round_coords(x) for x in obj]
    return obj


def feature(geom, props: dict) -> dict:
    g = mapping(geom)
    return {"type": "Feature", "properties": props,
            "geometry": {"type": g["type"], "coordinates": round_coords(g["coordinates"])}}


def write(name: str, features: list[dict]) -> None:
    path = OUT_DIR / name
    path.write_text(json.dumps({"type": "FeatureCollection", "features": features}, separators=(",", ":")))
    print(f"  {name}: {len(features)} features, {path.stat().st_size // 1024} KB")


def landcover_class(tags: dict) -> str | None:
    for key, values, cls in LANDCOVER_RULES:
        if tags.get(key) in values:
            return cls
    return None


def main() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    raw = json.loads((RAW_DIR / "osm.json").read_text())
    clip = box(*BBOX_WGS84)
    items = []
    for f in osm2geojson.json2geojson(raw)["features"]:
        if not f.get("geometry"):
            continue
        geom = shape(f["geometry"])
        if not geom.is_valid:
            geom = geom.buffer(0)
        geom = geom.intersection(clip)
        if not geom.is_empty:
            items.append((geom, f["properties"].get("tags", {})))

    paths, landcover, water, parks, amenity_points = [], [], [], [], []
    for geom, tags in items:
        polygonal = geom.geom_type in ("Polygon", "MultiPolygon")
        name = tags.get("name")
        if tags.get("highway") in PATH_HIGHWAYS and geom.geom_type in ("LineString", "MultiLineString"):
            if tags.get("footway") in STREET_FOOTWAYS:
                continue
            paths.append(feature(geom, {"highway": tags["highway"], "surface": tags.get("surface", "unknown"),
                                        **({"name": name} if name else {})}))
        elif (tags.get("natural") == "water" or "water" in tags) and polygonal:
            water.append(feature(geom, {"name": name} if name else {}))
        elif tags.get("leisure") in ("park", "nature_reserve", "common") and polygonal:
            parks.append(feature(geom, {"kind": tags["leisure"], **({"name": name} if name else {})}))
        elif tags.get("amenity") in AMENITIES:
            amenity_points.append((geom.representative_point(), tags))
        elif polygonal and (cls := landcover_class(tags)):
            landcover.append(feature(geom, {"class": cls, **({"name": name} if name else {})}))

    # Keep only amenities on the Heath and in its parks, not every café in the village.
    park_area = unary_union([shape(p["geometry"]) for p in parks]).buffer(0.0003)
    amenities = [feature(pt, {"amenity": t["amenity"], **({"name": t["name"]} if "name" in t else {})})
                 for pt, t in amenity_points if park_area.contains(pt)]

    print("Writing OSM layers...")
    write("paths.geojson", paths)
    write("landcover.geojson", landcover)
    write("water.geojson", water)
    write("parks.geojson", parks)
    write("amenities.geojson", amenities)


if __name__ == "__main__":
    main()
