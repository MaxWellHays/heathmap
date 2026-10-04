"""Export a level (terrain, ground texture, trees, buildings) for the 3D engine.

Run after build_buildings.py and build_trees.py. Everything is in local metres
around ORIGIN (a point in British National Grid), with the engine's axes:
  x = east, y = up (elevation), z = south   (Bevy is right-handed, Y-up; north is −z)

Outputs in engine/assets/levels/heath/ (generated, not committed):
  level.json     extents, resolutions, file names and the GPS → level transform (georef)
  terrain.bin    u16 little-endian heightmap, row-major from the north-west corner;
                 elevation = min_ele + value / 100 (centimetre steps)
  ground.png     top-down colour map covering the terrain extent (same orientation)
  trees.bin      f32 little-endian records: x, z, ground, height, crown_radius
  buildings.bin  footprints with LIDAR heights (engine builds walls and roofs)
  lines.bin      road and path centre lines (engine drapes them on the terrain)
  water.bin      pond and lake outlines (engine builds flat water surfaces)
  props.bin      benches, lamps, signals, bus stops, crossings, fountains… (oriented)
  barriers.bin   fences, walls, hedges, retaining walls
"""

import csv
import gzip
import json
import re
import struct
import subprocess

import numpy as np
import rasterio
from PIL import Image
from rasterio.enums import Resampling
from rasterio.features import rasterize
from rasterio.transform import from_origin
from shapely.geometry import box, shape
from shapely.ops import transform as shp_transform

from area import BBOX_BNG, BUILD_DIR, RAW_DIR, ROOT

LEVEL_DIR = ROOT.parent / "engine" / "assets" / "levels" / "heath"
ORIGIN = ((BBOX_BNG[0] + BBOX_BNG[2]) / 2, (BBOX_BNG[1] + BBOX_BNG[3]) / 2)  # BNG metres
TERRAIN_RES = 4.0  # metres per heightmap sample
GROUND_RES = 2.0  # metres per ground texture pixel (4000 × 3500 px; under WebGL2's 4096 limit)

# Ground colours, painted in this order (later entries cover earlier ones).
GROUND_LAYERS = [
    ("urban", "#dcd8d0"),
    ("park", "#b7cf95"),
    ("grass", "#bfd69c"),
    ("meadow", "#c9d99a"),
    ("scrub", "#9ab878"),
    ("heath", "#b9b077"),
    ("wood", "#7fa363"),
    ("cemetery", "#a9bd98"),
    ("pitch", "#a8d08a"),
    ("wetland", "#9cc7b4"),
    ("water", "#7fb2d6"),
    ("road", "#f4f2ee"),
    ("road_major", "#f6e7b8"),
    ("path_paved", "#d9d4ca"),
    ("path_unpaved", "#c4a87a"),
    ("building", "#c9c2b8"),
]
LAYER_INDEX = {name: i for i, (name, _) in enumerate(GROUND_LAYERS)}

LANDUSE_CLASSES = {  # (OSM key, value) -> ground layer
    ("leisure", "park"): "park", ("leisure", "nature_reserve"): "park", ("leisure", "common"): "park",
    ("leisure", "garden"): "grass", ("landuse", "grass"): "grass", ("landuse", "recreation_ground"): "grass",
    ("landuse", "village_green"): "grass", ("landuse", "meadow"): "meadow", ("natural", "grassland"): "meadow",
    ("natural", "scrub"): "scrub", ("natural", "heath"): "heath", ("natural", "wood"): "wood",
    ("landuse", "forest"): "wood", ("landuse", "cemetery"): "cemetery", ("leisure", "pitch"): "pitch",
    ("natural", "wetland"): "wetland", ("natural", "water"): "water", ("landuse", "reservoir"): "water",
}
ROAD_WIDTHS = {  # metres
    "motorway": 14, "trunk": 12, "primary": 11, "secondary": 9, "tertiary": 8,
    "residential": 6, "unclassified": 6, "living_street": 5, "service": 4, "pedestrian": 5,
}
MAJOR_ROADS = {"motorway", "trunk", "primary", "secondary"}
PATH_HIGHWAYS = {"footway", "path", "track", "bridleway", "cycleway", "steps"}
PAVED = {"asphalt", "paved", "concrete", "paving_stones", "sett", "concrete:plates", "chipseal"}


def polygons(g) -> list:
    """The polygons in any geometry: a Polygon, a MultiPolygon or a GeometryCollection (which
    clipping or repairing a footprint can produce, mixed with stray lines and points)."""
    if g.geom_type == "Polygon":
        return [g]
    return [p for part in getattr(g, "geoms", []) for p in polygons(part)]


def lines_of(g) -> list:
    """The line strings in any geometry (LineString, MultiLineString or GeometryCollection)."""
    if g.geom_type in ("LineString", "LinearRing"):
        return [g]
    return [p for part in getattr(g, "geoms", []) for p in lines_of(part)]


def to_local(x, y):
    """BNG metres -> engine (x, z)."""
    return x - ORIGIN[0], -(y - ORIGIN[1])


def ogr_features(layer: str, where: str, select: str) -> list[dict]:
    """Features from the OSM extract, in BNG, inside the level extent."""
    out = BUILD_DIR / f"engine_{layer}.geojson"
    out.unlink(missing_ok=True)
    e0, n0, e1, n1 = BBOX_BNG
    subprocess.run([
        "ogr2ogr", "-f", "GeoJSON", str(out), str(RAW_DIR / "greater-london.osm.pbf"), layer,
        "-where", where, "-select", select, "-t_srs", "EPSG:27700",
        "-spat", str(e0), str(n0), str(e1), str(n1), "-spat_srs", "EPSG:27700",
    ], check=True, stderr=subprocess.DEVNULL)
    return json.loads(out.read_text())["features"]


def tag(other_tags: str | None, key: str) -> str | None:
    m = re.search(rf'"{re.escape(key)}"=>"([^"]*)"', other_tags or "")
    return m.group(1) if m else None


def export_terrain(meta: dict) -> None:
    e0, n0, e1, n1 = BBOX_BNG
    width, height = int((e1 - e0) / TERRAIN_RES), int((n1 - n0) / TERRAIN_RES)
    with rasterio.open(BUILD_DIR / "dtm_3857.tif"):  # ensure terrain build ran (gap filling lives there)
        pass
    with rasterio.open(BUILD_DIR / "dtm_1m.vrt") as src:
        window = rasterio.windows.from_bounds(e0, n0, e1, n1, src.transform)
        ele = src.read(1, window=window, out_shape=(height, width), resampling=Resampling.average, masked=True)
    ele = ele.filled(float(ele.mean()))
    lo = float(np.floor(ele.min()))
    values = np.round((ele - lo) * 100).clip(0, 65535).astype("<u2")
    (LEVEL_DIR / "terrain.bin").write_bytes(values.tobytes())
    meta["terrain"] = {"file": "terrain.bin", "width": width, "height": height, "resolution": TERRAIN_RES,
                       "min_ele": lo, "max_ele": round(float(ele.max()), 2)}
    print(f"  terrain: {width}×{height} samples at {TERRAIN_RES} m, {lo:.0f}–{ele.max():.0f} m")


def export_ground(meta: dict) -> None:
    e0, n0, e1, n1 = BBOX_BNG
    width, height = int((e1 - e0) / GROUND_RES), int((n1 - n0) / GROUND_RES)
    transform = from_origin(e0, n1, GROUND_RES, GROUND_RES)
    shapes: list[tuple] = []

    polys = ogr_features("multipolygons", "landuse IS NOT NULL OR natural IS NOT NULL OR leisure IS NOT NULL",
                         "landuse,natural,leisure")
    for f in polys:
        p = f["properties"]
        for key in ("natural", "landuse", "leisure"):
            cls = LANDUSE_CLASSES.get((key, p.get(key)))
            if cls:
                shapes.append((LAYER_INDEX[cls], shape(f["geometry"])))
                break

    lines = ogr_features("lines", "highway IS NOT NULL", "highway,other_tags")
    for f in lines:
        hw = f["properties"]["highway"]
        g = shape(f["geometry"])
        if hw in ROAD_WIDTHS:
            layer = "road_major" if hw in MAJOR_ROADS else "road"
            shapes.append((LAYER_INDEX[layer], g.buffer(ROAD_WIDTHS[hw] / 2, cap_style="flat")))
        elif hw in PATH_HIGHWAYS:
            surface = tag(f["properties"].get("other_tags"), "surface")
            layer = "path_paved" if surface in PAVED else "path_unpaved"
            shapes.append((LAYER_INDEX[layer], g.buffer(1.0, cap_style="flat")))

    for f in json.loads((BUILD_DIR / "buildings.geojson").read_text())["features"]:
        shapes.append((LAYER_INDEX["building"], shape(f["geometry"])))

    # Paint in layer order so higher layers win.
    shapes.sort(key=lambda s: s[0])
    grid = np.zeros((height, width), dtype=np.uint8)  # 0 = urban
    for idx in sorted({s[0] for s in shapes}):
        burn = [(g, 1) for i, g in shapes if i == idx and not g.is_empty]
        mask = rasterize(burn, out_shape=grid.shape, transform=transform, dtype="uint8").astype(bool)
        grid[mask] = idx

    palette = np.array([[int(c[i:i + 2], 16) for i in (1, 3, 5)] for _, c in GROUND_LAYERS], dtype=np.uint8)
    Image.fromarray(palette[grid], "RGB").save(LEVEL_DIR / "ground.png", optimize=True)
    meta["ground"] = {"file": "ground.png", "width": width, "height": height, "resolution": GROUND_RES}
    print(f"  ground: {width}×{height} px at {GROUND_RES} m")


# Ground classes for the signed distance fields, in paint order (later covers earlier).
# Must match engine/src/ground.rs. Three RGBA textures hold 4 classes each.
SDF_CLASSES = ["park", "grass", "meadow", "scrub", "heath", "wood",
               "cemetery", "pitch", "wetland", "water", "road", "road_major"]
SDF_RES = 4.0  # metres per texel; edges stay sharp, corners round at about this scale
SDF_FINE = 1.0  # metres per pixel for the masks the distances are computed from
SDF_SCALE = 8.0  # texel value steps per metre (128 = on the edge; ±16 m range)


def export_ground_sdf(meta: dict) -> None:
    """Signed distance to each ground class's boundary (negative inside), for sharp edges.

    The engine's terrain shader thresholds these per pixel, so area edges stay crisp at
    any zoom (the technique used for sharp text in games), unlike a colour texture.
    Format: raw u8, len(SDF_CLASSES)/4 layers of width × height × RGBA, rows from the north.
    """
    from scipy.ndimage import distance_transform_edt

    e0, n0, e1, n1 = BBOX_BNG
    fine_w, fine_h = int((e1 - e0) / SDF_FINE), int((n1 - n0) / SDF_FINE)
    step = int(SDF_RES / SDF_FINE)
    out_w, out_h = fine_w // step, fine_h // step
    transform = from_origin(e0, n1, SDF_FINE, SDF_FINE)

    shapes: dict[str, list] = {c: [] for c in SDF_CLASSES}
    for f in ogr_features("multipolygons", "landuse IS NOT NULL OR natural IS NOT NULL OR leisure IS NOT NULL",
                          "landuse,natural,leisure"):
        p = f["properties"]
        for key in ("natural", "landuse", "leisure"):
            cls = LANDUSE_CLASSES.get((key, p.get(key)))
            if cls:
                shapes[cls].append(shape(f["geometry"]))
                break
    for f in ogr_features("lines", "highway IS NOT NULL", "highway,other_tags"):
        hw = f["properties"]["highway"]
        if hw in ROAD_WIDTHS:
            cls = "road_major" if hw in MAJOR_ROADS else "road"
            shapes[cls].append(shape(f["geometry"]).buffer(ROAD_WIDTHS[hw] / 2, cap_style="flat"))

    layers = np.zeros((len(SDF_CLASSES) // 4, out_h, out_w, 4), dtype=np.uint8)
    centre = step // 2  # sample the fine distances at coarse texel centres
    for i, cls in enumerate(SDF_CLASSES):
        geoms = [g for g in shapes[cls] if not g.is_empty]
        if geoms:
            mask = rasterize(((g, 1) for g in geoms), out_shape=(fine_h, fine_w), transform=transform,
                             dtype="uint8").astype(bool)
            outside = distance_transform_edt(~mask)[centre::step, centre::step]
            inside = distance_transform_edt(mask)[centre::step, centre::step]
            signed = (outside - inside) * SDF_FINE
            del mask
        else:
            signed = np.full((out_h, out_w), 999.0)
        layers[i // 4, :, :, i % 4] = np.clip(np.round(128 + signed[:out_h, :out_w] * SDF_SCALE), 0, 255)
        print(f"    sdf {cls}: {len(geoms)} shapes")
    (LEVEL_DIR / "ground_sdf.bin").write_bytes(layers.tobytes())
    meta["ground_sdf"] = {"file": "ground_sdf.bin", "width": out_w, "height": out_h, "resolution": SDF_RES,
                          "layers": len(SDF_CLASSES) // 4, "scale": SDF_SCALE, "classes": SDF_CLASSES}
    print(f"  ground sdf: {len(SDF_CLASSES)} classes, {out_w}×{out_h} at {SDF_RES} m")


TRUNK_CLEARANCE = 0.4  # metres a moved trunk ends up outside the road or path edge


def path_areas() -> list:
    """Road and path surfaces (centre lines buffered to their width), in BNG."""
    areas = []
    for f in ogr_features("lines", "highway IS NOT NULL", "highway,other_tags"):
        hw = f["properties"]["highway"]
        if hw in ROAD_WIDTHS:
            width = ROAD_WIDTHS[hw]
        elif hw in PATH_HIGHWAYS:
            if tag(f["properties"].get("other_tags"), "footway") in ("sidewalk", "crossing"):
                continue
            width = {"track": 3.0, "steps": 2.0}.get(hw, 1.5)
        else:
            continue
        areas.append(shape(f["geometry"]).buffer(width / 2))
    return areas


def export_trees(meta: dict) -> None:
    """Trees, with trunks that fall on a road or path moved just outside it.

    LIDAR tree tops are often over paths (crowns overhang them), but a trunk in the
    middle of a path looks wrong and blocks walking; the crown stays where it was.
    """
    from shapely import STRtree
    from shapely.geometry import Point
    from shapely.ops import nearest_points, unary_union

    rows = list(csv.DictReader((BUILD_DIR / "trees_lidar.csv").open()))
    areas = path_areas()
    index = STRtree(areas)
    buildings = [shape(f["geometry"]).buffer(0.5) for f in json.loads((BUILD_DIR / "buildings.geojson").read_text())["features"]]
    building_index = STRtree(buildings)
    in_building = lambda q: len(building_index.query(q, predicate="intersects")) > 0
    data = np.empty((len(rows), 5), dtype="<f4")
    moved = kept = 0
    for i, r in enumerate(rows):
        bx, by = float(r["x"]), float(r["y"])
        p = Point(bx, by)
        hits = [areas[j] for j in index.query(p, predicate="intersects")]
        if hits:
            region = unary_union(hits)
            # Push to the nearest point on the region's edge, plus a little clearance. Never
            # into a building: then the tree stays put (street trees on narrow pavements).
            edge = nearest_points(region.boundary, p)[0]
            direction = np.array([edge.x - bx, edge.y - by])
            norm = np.linalg.norm(direction)
            if norm > 1e-6:
                nx, ny = edge.x + direction[0] / norm * TRUNK_CLEARANCE, edge.y + direction[1] / norm * TRUNK_CLEARANCE
                if in_building(Point(nx, ny)):
                    kept += 1
                else:
                    bx, by = nx, ny
                    moved += 1
        x, z = to_local(bx, by)
        data[i] = (x, z, float(r["ground"]), float(r["height"]), float(r["crown_r"]))
    print(f"    moved {moved} trunks off roads and paths ({kept} left in place: a building was in the way)")
    (LEVEL_DIR / "trees.bin").write_bytes(data.tobytes())
    meta["trees"] = {"file": "trees.bin", "count": len(rows), "fields": ["x", "z", "ground", "height", "crown_r"]}
    print(f"  trees: {len(rows)}")


LANDMARKS = json.loads((ROOT / "landmarks.json").read_text())
LANDMARK_IDS = {lm["osm_id"] for lm in LANDMARKS}
LANDMARK_GENERATORS = {"pergola": 0}


def export_buildings(meta: dict) -> None:
    """Footprints with LIDAR heights; the engine extrudes walls and builds roofs.

    Format (little-endian): u32 count, then per building:
      f32 ground, f32 height (p90 above ground), f32 height_p50, u16 ring count,
      per ring: u32 vertex count, vertex count × (f32 x, f32 z). Ring 0 is the outer ring.
    """
    features = json.loads((BUILD_DIR / "buildings.geojson").read_text())["features"]
    out = bytearray(struct.pack("<I", 0))
    count = 0
    for f in features:
        p = f["properties"]
        if p.get("osm_id") in LANDMARK_IDS:
            continue  # built by its own generator (export_landmarks)
        for poly in polygons(shape(f["geometry"])):
            if poly.area < 4:
                continue
            rings = [poly.exterior, *poly.interiors]
            out += struct.pack("<fffH", p["ground"], p["height"], p.get("height_p50", p["height"]), len(rings))
            for ring in rings:
                pts = list(ring.coords)[:-1]  # drop the closing duplicate
                out += struct.pack("<I", len(pts))
                out += struct.pack(f"<{2 * len(pts)}f", *(v for x, y in pts for v in to_local(x, y)))
            count += 1
    struct.pack_into("<I", out, 0, count)
    (LEVEL_DIR / "buildings.bin").write_bytes(out)
    meta["buildings"] = {"file": "buildings.bin", "count": count}
    print(f"  buildings: {count}")


def export_water(meta: dict) -> None:
    """Ponds, lakes and reservoirs; the engine builds flat water surfaces from them.

    Format (little-endian): u32 count, then per polygon: u16 ring count, per ring:
    u32 vertex count, vertex count × (f32 x, f32 z). Ring 0 is the outer ring.
    """
    polys = ogr_features("multipolygons", "natural = 'water' OR landuse = 'reservoir' OR other_tags LIKE '%\"water\"=>%'",
                         "natural,landuse")
    # Ponds are often mapped twice (a tagged way and a multipolygon relation, or water=*
    # inside natural=water); merge overlaps so each water body becomes one surface.
    from shapely.ops import unary_union
    merged = unary_union([shape(f["geometry"]).buffer(0) for f in polys])
    out = bytearray(struct.pack("<I", 0))
    count = 0
    for g in [merged]:
        for poly in polygons(g):
            if poly.area < 10:
                continue
            rings = [poly.exterior, *poly.interiors]
            out += struct.pack("<H", len(rings))
            for ring in rings:
                pts = list(ring.coords)[:-1]
                out += struct.pack("<I", len(pts))
                out += struct.pack(f"<{2 * len(pts)}f", *(v for x, y in pts for v in to_local(x, y)))
            count += 1
    struct.pack_into("<I", out, 0, count)
    (LEVEL_DIR / "water.bin").write_bytes(out)
    meta["water"] = {"file": "water.bin", "count": count}
    print(f"  water: {count} polygons")


def export_landmarks(meta: dict) -> None:
    """Landmarks with their own generators (data/landmarks.json), e.g. the Hampstead Pergola.

    Format (little-endian): u32 count, then per landmark: u8 generator, outline (u32 vertex
    count + (f32 x, f32 z) each), u16 walkway count, per walkway: u32 vertex count + vertices.
    Walkways are the footpaths inside the outline (clipped), along which columns are placed.
    """
    from shapely.geometry import LineString
    buildings = {f["properties"]["osm_id"]: f for f in json.loads((BUILD_DIR / "buildings.geojson").read_text())["features"]}
    paths = [shape(f["geometry"]) for f in ogr_features("lines", "highway IS NOT NULL", "highway,other_tags")
             if f["properties"]["highway"] in PATH_HIGHWAYS - {"steps"} | {"pedestrian"}]
    out = bytearray(struct.pack("<I", 0))
    count = 0
    for lm in LANDMARKS:
        f = buildings.get(lm["osm_id"])
        if f is None:
            print(f"    landmark {lm['name']} not found in buildings")
            continue
        g = shape(f["geometry"])
        poly = max(polygons(g), key=lambda q: q.area)
        ring = list(poly.exterior.coords)[:-1]
        walks = []
        for line in paths:
            inside = line.intersection(poly)
            for part in getattr(inside, "geoms", [inside]):
                if isinstance(part, LineString) and part.length > 3:
                    walks.append(list(part.coords))
        out += struct.pack("<BI", LANDMARK_GENERATORS[lm["generator"]], len(ring))
        out += struct.pack(f"<{2 * len(ring)}f", *(v for x, y in ring for v in to_local(x, y)))
        out += struct.pack("<H", len(walks))
        for w in walks:
            out += struct.pack("<I", len(w)) + struct.pack(f"<{2 * len(w)}f", *(v for x, y in w for v in to_local(x, y)))
        count += 1
        print(f"    {lm['name']}: {len(ring)} outline vertices, {len(walks)} walkways")
    struct.pack_into("<I", out, 0, count)
    (LEVEL_DIR / "landmarks.bin").write_bytes(out)
    meta["landmarks"] = {"file": "landmarks.bin", "count": count}


GEOREF_TERMS = [(i, j) for i in range(4) for j in range(4 - i)]  # cubic in (lon, lat)


def export_georef(meta: dict) -> None:
    """How to place GPS points (WGS84) in the level, for runs imported in the engine.

    A cubic polynomial in (lon − lon0, lat − lat0) × 100 fitted to the exact WGS84 → BNG
    transform over the level and a few km around it (error well under a millimetre), so the
    engine needs no projection library: x = Σ x[k] u^i v^j, z = Σ z[k] u^i v^j over `terms`.
    """
    from pyproj import Transformer
    to_wgs = Transformer.from_crs("EPSG:27700", "EPSG:4326", always_xy=True)
    to_bng = Transformer.from_crs("EPSG:4326", "EPSG:27700", always_xy=True)
    margin = 3000
    e, n = np.meshgrid(np.linspace(BBOX_BNG[0] - margin, BBOX_BNG[2] + margin, 60),
                       np.linspace(BBOX_BNG[1] - margin, BBOX_BNG[3] + margin, 60))
    lon, lat = to_wgs.transform(e.ravel(), n.ravel())
    lon0, lat0 = to_wgs.transform(*ORIGIN)
    xs, ys = to_bng.transform(lon, lat)
    x, z = to_local(np.asarray(xs), np.asarray(ys))
    u, v = (np.asarray(lon) - lon0) * 100, (np.asarray(lat) - lat0) * 100
    a = np.stack([u**i * v**j for i, j in GEOREF_TERMS], axis=1)
    cx, cz = (np.linalg.lstsq(a, t, rcond=None)[0] for t in (x, z))
    error = max(np.abs(a @ cx - x).max(), np.abs(a @ cz - z).max())
    meta["georef"] = {"lon0": lon0, "lat0": lat0, "scale": 100, "terms": GEOREF_TERMS,
                      "x": cx.tolist(), "z": cz.tolist()}
    print(f"  georef: max error {error * 1000:.3f} mm")


# Prop kinds shared with the engine (engine/src/props.rs).
PROP_KINDS = {"bench": 0, "drinking_water": 1, "street_lamp": 2, "traffic_signals": 3, "bus_stop": 4,
              "waste_basket": 5, "post_box": 6, "picnic_table": 7, "crossing": 8, "telephone": 9}
ZEBRA_CROSSINGS = {"zebra", "uncontrolled", "marked"}


def export_props(meta: dict) -> None:
    """Street furniture and other small objects from OSM points, oriented sensibly.

    Benches face the nearest path; traffic signals mapped on a road's centre line move to
    its edge; bus stops face the road; crossings take the road's direction and width.
    Format (little-endian): u32 count, then per prop:
      u8 kind, u8 variant (bus stop: 1 = shelter), f32 x, f32 z, f32 yaw, f32 size.
    yaw: rotation about the vertical so the prop's front (−z) faces that way;
    size: crossing length across the road (m), otherwise 0.
    """
    from shapely import STRtree
    from shapely.geometry import Point
    from shapely.ops import nearest_points

    lines, widths, is_road = [], [], []
    for f in ogr_features("lines", "highway IS NOT NULL", "highway,other_tags"):
        hw = f["properties"]["highway"]
        if hw in ROAD_WIDTHS or hw in PATH_HIGHWAYS:
            lines.append(shape(f["geometry"]))
            widths.append(ROAD_WIDTHS.get(hw, 1.5))
            is_road.append(hw in ROAD_WIDTHS and hw != "pedestrian")
    road_idx = [i for i, r in enumerate(is_road) if r]
    all_tree, road_tree = STRtree(lines), STRtree([lines[i] for i in road_idx])

    def tangent(line, p):
        d = line.project(p)
        a, b = line.interpolate(max(d - 1.0, 0)), line.interpolate(min(d + 1.0, line.length))
        v = np.array([b.x - a.x, b.y - a.y])
        n = np.linalg.norm(v)
        return v / n if n > 1e-6 else np.array([1.0, 0.0])

    def yaw_towards(dx_bng, dy_bng):
        # BNG direction -> engine yaw (x east, z south; front is −z).
        dx, dz = dx_bng, -dy_bng
        return float(np.arctan2(-dx, -dz))

    out = bytearray(struct.pack("<I", 0))
    count = 0
    for f in ogr_features("points", "highway IS NOT NULL OR other_tags IS NOT NULL", "highway,other_tags"):
        p = f["properties"]
        tags = p.get("other_tags")
        hw = p.get("highway")
        amenity = tag(tags, "amenity")
        if amenity in ("bench", "drinking_water", "waste_basket", "post_box", "telephone"):
            kind = amenity
        elif tag(tags, "leisure") == "picnic_table":
            kind = "picnic_table"
        elif hw in ("street_lamp", "traffic_signals", "bus_stop"):
            kind = hw
        elif hw == "crossing" and (tag(tags, "crossing") in ZEBRA_CROSSINGS or tag(tags, "crossing_ref") == "zebra"):
            kind = "crossing"
        else:
            continue
        pt = Point(f["geometry"]["coordinates"])
        x, y = pt.x, pt.y
        yaw, size, variant = 0.0, 0.0, 0
        tree, idx_map = (road_tree, road_idx) if kind in ("traffic_signals", "bus_stop", "crossing") else (all_tree, None)
        nearest = tree.nearest(pt)
        if nearest is not None:
            li = idx_map[nearest] if idx_map else nearest
            line, width = lines[li], widths[li]
            on_line = nearest_points(line, pt)[0]
            dist = pt.distance(on_line)
            t = tangent(line, on_line)
            if kind == "crossing":
                if dist < 3.0:
                    yaw = yaw_towards(*t)  # stripes run along the road
                    size = width
                else:
                    continue
            elif dist < 12.0:
                to_line = np.array([on_line.x - x, on_line.y - y])
                if kind == "traffic_signals" and dist < width / 2:
                    # Mapped in the road: move to its edge, on the side the point is on.
                    side = np.array([-t[1], t[0]])
                    if to_line.dot(side) > 0:
                        side = -side
                    x, y = on_line.x + side[0] * (width / 2 + 0.4), on_line.y + side[1] * (width / 2 + 0.4)
                    to_line = np.array([on_line.x - x, on_line.y - y])
                if np.linalg.norm(to_line) > 1e-3:
                    yaw = yaw_towards(*to_line)  # face the path / road
                else:
                    yaw = yaw_towards(-t[1], t[0])
        if kind == "bus_stop" and tag(tags, "shelter") == "yes":
            variant = 1
        lx, lz = to_local(x, y)
        out += struct.pack("<BBffff", PROP_KINDS[kind], variant, lx, lz, yaw, size)
        count += 1
    struct.pack_into("<I", out, 0, count)
    (LEVEL_DIR / "props.bin").write_bytes(out)
    meta["props"] = {"file": "props.bin", "count": count, "kinds": PROP_KINDS}
    print(f"  props: {count}")


# Barrier kinds and materials shared with the engine (engine/src/props.rs).
BARRIER_KINDS = {"fence": 0, "wire_fence": 0, "wall": 1, "city_wall": 1, "hedge": 2, "retaining_wall": 3}
BARRIER_MATERIALS = {"metal": 1, "steel": 1, "iron": 1, "wood": 2, "brick": 3, "stone": 4, "concrete": 5}


def export_barriers(meta: dict) -> None:
    """Fences, walls, hedges and retaining walls (OSM barrier=*), lines and area outlines.

    Format (little-endian): u32 count, then per barrier: u8 kind, u8 material,
    f32 height (m, 0 = default for the kind), u32 vertex count, vertex count × (f32 x, f32 z).
    """
    out = bytearray(struct.pack("<I", 0))
    count = 0
    sources = [("lines", "barrier IS NOT NULL", "barrier,other_tags"),
               ("multipolygons", "barrier IS NOT NULL", "barrier,other_tags")]
    for layer, where, select in sources:
        for f in ogr_features(layer, where, select):
            p = f["properties"]
            kind = BARRIER_KINDS.get(p.get("barrier"))
            if kind is None:
                continue
            tags = p.get("other_tags")
            material = BARRIER_MATERIALS.get(tag(tags, "material") or tag(tags, "fence_type") or "", 0)
            if p["barrier"] == "wire_fence" or (tag(tags, "fence_type") or "").startswith(("metal", "railing", "chain")):
                material = 1
            try:
                height = float((tag(tags, "height") or "0").rstrip(" m"))
            except ValueError:
                height = 0.0
            g = shape(f["geometry"])
            parts = [poly.exterior for poly in polygons(g)] + lines_of(g)
            for part in parts:
                pts = list(part.coords)
                if len(pts) < 2:
                    continue
                out += struct.pack("<BBfI", kind, material, min(height, 8.0), len(pts))
                out += struct.pack(f"<{2 * len(pts)}f", *(v for x, y in pts for v in to_local(x, y)))
                count += 1
    struct.pack_into("<I", out, 0, count)
    (LEVEL_DIR / "barriers.bin").write_bytes(out)
    meta["barriers"] = {"file": "barriers.bin", "count": count}
    print(f"  barriers: {count}")


# Line kinds shared with the engine (engine/src/lines.rs).
LINE_KINDS = {"road_major": 0, "road_minor": 1, "service": 2, "pedestrian": 3,
              "path_paved": 4, "path_unpaved": 5, "steps": 6, "track": 7}
MINOR_ROADS = {"tertiary", "residential", "unclassified", "living_street"}


def export_lines(meta: dict) -> None:
    """Road and path centre lines; the engine drapes them on the terrain as ribbons.

    Format (little-endian): u32 count, then per line:
      u8 kind, u8 flags (1 = oneway, 2 = bridge), u8 lanes (0 = unknown), f32 width (m),
      u32 vertex count, vertex count × (f32 x, f32 z).
    """
    lines = ogr_features("lines", "highway IS NOT NULL", "highway,other_tags")
    out = bytearray(struct.pack("<I", 0))
    count = 0
    for f in lines:
        hw = f["properties"]["highway"]
        tags = f["properties"].get("other_tags")
        if tag(tags, "tunnel") in ("yes", "building_passage") or tag(tags, "layer") in ("-1", "-2"):
            continue
        surface = tag(tags, "surface")
        if hw in MAJOR_ROADS:
            kind = "road_major"
        elif hw in MINOR_ROADS:
            kind = "road_minor"
        elif hw == "service":
            kind = "service"
        elif hw == "pedestrian":
            kind = "pedestrian"
        elif hw == "steps":
            kind = "steps"
        elif hw == "track":
            kind = "track"
        elif hw in PATH_HIGHWAYS:
            if tag(tags, "footway") in ("sidewalk", "crossing"):
                continue  # drawn as part of the road
            kind = "path_paved" if surface in PAVED else "path_unpaved"
        else:
            continue
        lanes_tag = tag(tags, "lanes")
        lanes = int(lanes_tag) if lanes_tag and lanes_tag.isdigit() else 0
        width_tag = tag(tags, "width")
        try:
            width = float(width_tag.rstrip(" m")) if width_tag else 0.0
        except ValueError:
            width = 0.0
        if not width:
            width = (lanes * 3.2 + 1.0) if lanes and kind.startswith("road") else \
                ROAD_WIDTHS.get(hw, {"track": 3.0, "steps": 2.0, "path_paved": 2.0}.get(kind, 1.5))
        flags = (1 if tag(tags, "oneway") == "yes" else 0) | (2 if tag(tags, "bridge") else 0)
        g = shape(f["geometry"])
        for line in lines_of(g):
            pts = list(line.coords)
            if len(pts) < 2:
                continue
            out += struct.pack("<BBBfI", LINE_KINDS[kind], flags, min(lanes, 255), width, len(pts))
            out += struct.pack(f"<{2 * len(pts)}f", *(v for x, y in pts for v in to_local(x, y)))
            count += 1
    struct.pack_into("<I", out, 0, count)
    (LEVEL_DIR / "lines.bin").write_bytes(out)
    meta["lines"] = {"file": "lines.bin", "count": count, "kinds": LINE_KINDS}
    print(f"  lines: {count}")


def compress_bins() -> None:
    """Gzips the .bin files in place (the engine detects gzip), so the web build downloads
    quickly from a static host that doesn't compress them itself (GitHub Pages)."""
    for p in sorted(LEVEL_DIR.glob("*.bin")):
        data = p.read_bytes()
        if not data.startswith(b"\x1f\x8b"):
            p.write_bytes(gzip.compress(data, compresslevel=6, mtime=0))


def main() -> None:
    LEVEL_DIR.mkdir(parents=True, exist_ok=True)
    e0, n0, e1, n1 = BBOX_BNG
    x0, z0 = to_local(e0, n1)  # north-west corner
    x1, z1 = to_local(e1, n0)
    meta = {"name": "Hampstead Heath", "origin_bng": ORIGIN, "extent": {"x": [x0, x1], "z": [z0, z1]}}
    export_terrain(meta)
    export_ground(meta)
    export_ground_sdf(meta)
    export_trees(meta)
    export_buildings(meta)
    export_landmarks(meta)
    export_lines(meta)
    export_water(meta)
    export_props(meta)
    export_barriers(meta)
    export_georef(meta)
    (LEVEL_DIR / "runs.bin").unlink(missing_ok=True)  # runs used to be baked in; now imported in the engine
    (LEVEL_DIR / "level.json").write_text(json.dumps(meta, indent=2) + "\n")
    compress_bins()
    sizes = {p.name: f"{p.stat().st_size / 1e6:.1f} MB" for p in sorted(LEVEL_DIR.iterdir())}
    print(f"Wrote {LEVEL_DIR}: {sizes}")


if __name__ == "__main__":
    main()
