"""Export a level (terrain, ground texture, trees, buildings) for the 3D engine.

Run after build_buildings.py and build_trees.py. Everything is in local metres
around ORIGIN (a point in British National Grid), with the engine's axes:
  x = east, y = up (elevation), z = south   (Bevy is right-handed, Y-up; north is −z)

Outputs in engine/assets/levels/heath/ (generated, not committed):
  level.json     extents, resolutions and file names
  terrain.bin    u16 little-endian heightmap, row-major from the north-west corner;
                 elevation = min_ele + value / 100 (centimetre steps)
  ground.png     top-down colour map covering the terrain extent (same orientation)
  trees.bin      f32 little-endian records: x, z, ground, height, crown_radius
  buildings.glb  extruded footprints, one mesh per 1 km chunk, flat roofs
"""

import csv
import json
import re
import subprocess

import numpy as np
import rasterio
import trimesh
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
CHUNK = 1000  # metres, building mesh chunks

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


def export_trees(meta: dict) -> None:
    rows = list(csv.DictReader((BUILD_DIR / "trees_lidar.csv").open()))
    data = np.empty((len(rows), 5), dtype="<f4")
    for i, r in enumerate(rows):
        x, z = to_local(float(r["x"]), float(r["y"]))
        data[i] = (x, z, float(r["ground"]), float(r["height"]), float(r["crown_r"]))
    (LEVEL_DIR / "trees.bin").write_bytes(data.tobytes())
    meta["trees"] = {"file": "trees.bin", "count": len(rows), "fields": ["x", "z", "ground", "height", "crown_r"]}
    print(f"  trees: {len(rows)}")


def export_buildings(meta: dict) -> None:
    features = json.loads((BUILD_DIR / "buildings.geojson").read_text())["features"]
    chunks: dict[tuple[int, int], list[trimesh.Trimesh]] = {}
    for f in features:
        g = shp_transform(lambda x, y, z=None: to_local(x, y), shape(f["geometry"]))
        p = f["properties"]
        polys = list(g.geoms) if g.geom_type == "MultiPolygon" else [g]
        for poly in polys:
            if poly.area < 4:
                continue
            try:
                m = trimesh.creation.extrude_polygon(poly.buffer(0), height=p["height"])
            except Exception:
                continue
            # extrude_polygon builds in (x, y, up); engine wants (x, up, z) with z = the polygon's y.
            v = m.vertices
            m.vertices = np.column_stack([v[:, 0], v[:, 2] + p["ground"], v[:, 1]])
            m.invert()  # swapping axes mirrors the mesh; restore outward-facing winding
            c = poly.centroid
            key = (int(np.floor(c.x / CHUNK)), int(np.floor(c.y / CHUNK)))
            chunks.setdefault(key, []).append(m)
    scene = trimesh.Scene()
    for (cx, cz), meshes in sorted(chunks.items()):
        merged = trimesh.util.concatenate(meshes)
        merged.visual = trimesh.visual.ColorVisuals(merged, face_colors=[214, 205, 192, 255])
        scene.add_geometry(merged, node_name=f"buildings_{cx}_{cz}", geom_name=f"buildings_{cx}_{cz}")
    scene.export(LEVEL_DIR / "buildings.glb")
    tris = sum(len(g.faces) for g in scene.geometry.values())
    meta["buildings"] = {"file": "buildings.glb", "count": len(features), "chunks": len(chunks), "triangles": tris}
    print(f"  buildings: {len(features)} in {len(chunks)} chunks, {tris} triangles")


def main() -> None:
    LEVEL_DIR.mkdir(parents=True, exist_ok=True)
    e0, n0, e1, n1 = BBOX_BNG
    x0, z0 = to_local(e0, n1)  # north-west corner
    x1, z1 = to_local(e1, n0)
    meta = {"name": "Hampstead Heath", "origin_bng": ORIGIN, "extent": {"x": [x0, x1], "z": [z0, z1]}}
    export_terrain(meta)
    export_ground(meta)
    export_trees(meta)
    export_buildings(meta)
    (LEVEL_DIR / "level.json").write_text(json.dumps(meta, indent=2) + "\n")
    sizes = {p.name: f"{p.stat().st_size / 1e6:.1f} MB" for p in sorted(LEVEL_DIR.iterdir())}
    print(f"Wrote {LEVEL_DIR}: {sizes}")


if __name__ == "__main__":
    main()
