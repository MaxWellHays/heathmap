"""Detect individual trees from LIDAR (run build_buildings.py first).

Method: the normalised surface (DSM − DTM) with buildings masked out is a canopy
height model. Local maxima of a lightly smoothed canopy are candidate tops; a
height-dependent window then drops candidates that sit inside a taller tree's
expected crown (so big crowns aren't split), and a watershed from the remaining
tops splits the canopy into crowns. Each tree gets its top position,
height (crown maximum) and crown radius (from crown area).

Processed in 1km tiles with an overlap, keeping only tops inside each tile's core,
so crowns crossing tile edges are segmented correctly and counted once.

Output: build/trees_lidar.csv — x, y (BNG metres), ground (m), height (m), crown_r (m)
"""

import csv
import json

import numpy as np
import rasterio
from rasterio.features import rasterize
from rasterio.windows import Window
from scipy import ndimage
from scipy.spatial import cKDTree
from shapely.geometry import shape
from skimage.feature import peak_local_max
from skimage.segmentation import watershed

from area import BUILD_DIR

MIN_HEIGHT = 3.0  # metres; lower vegetation is shrubs/hedges, not trees
SMOOTH_SIGMA = 0.8  # pixels (1 m); suppresses branch-level bumps that would split crowns
MIN_DISTANCE = 2  # pixels; candidate spacing before the height-dependent suppression
MIN_CROWN_AREA = 3  # m²; smaller blobs are lamp posts, vehicles or noise
MAX_HEIGHT = 40.0  # metres; taller "trees" are cranes or buildings missing from OSM
WINDOW_SCALE = 2.0  # multiplier on the crown-width model (fitted to forest trees; London's open-grown
#                    crowns are wider). Tuned against Trees Outside Woodland crowns.
BUILDING_BUFFER = 1.5  # metres around footprints excluded (walls, eaves, mapping offsets)
TILE = 1000
OVERLAP = 30


def building_mask(shape_, transform) -> np.ndarray:
    features = json.loads((BUILD_DIR / "buildings.geojson").read_text())["features"]
    geoms = (shape(f["geometry"]).buffer(BUILDING_BUFFER) for f in features)
    return rasterize(((g, 1) for g in geoms), out_shape=shape_, transform=transform, dtype="uint8").astype(bool)


def crown_window_radius(height: np.ndarray) -> np.ndarray:
    """Expected crown radius for a tree of this height (m).

    Variable-window model for deciduous trees from Popescu & Wynne (2004):
    crown width = 2.51503 + 0.00901 · h². Tall trees get a wide window, so their own
    branches are not mistaken for neighbouring trees; small trees keep a tight one.
    """
    return WINDOW_SCALE * (2.51503 + 0.00901 * height**2) / 2


def suppress(tops: np.ndarray, smooth: np.ndarray) -> np.ndarray:
    """Keeps a candidate top only if no taller kept top lies within the taller tree's crown radius."""
    h = smooth[tops[:, 0], tops[:, 1]]
    order = np.argsort(-h)
    tree = cKDTree(tops)
    removed = np.zeros(len(tops), dtype=bool)
    for i in order:
        if removed[i]:
            continue
        for j in tree.query_ball_point(tops[i], crown_window_radius(h[i])):
            if j != i and h[j] <= h[i]:
                removed[j] = True
    return tops[~removed]


def detect(chm: np.ndarray) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """Returns (row, col) of tops, heights and crown areas for one tile."""
    smooth = ndimage.gaussian_filter(chm, SMOOTH_SIGMA)
    canopy = chm >= MIN_HEIGHT
    # Height threshold applies to the raw canopy (via `labels`): smoothing lowers the peaks
    # of small crowns, and thresholding the smoothed surface would drop young trees.
    candidates = peak_local_max(smooth, min_distance=MIN_DISTANCE, labels=canopy.astype(np.uint8),
                                exclude_border=False)
    tops = suppress(candidates, smooth)
    markers = np.zeros(chm.shape, dtype=np.int32)
    markers[tops[:, 0], tops[:, 1]] = np.arange(1, len(tops) + 1)
    crowns = watershed(-smooth, markers, mask=canopy)
    index = np.arange(1, len(tops) + 1)
    heights = ndimage.maximum(chm, crowns, index)
    areas = ndimage.sum(np.ones_like(chm), crowns, index)
    return tops, np.asarray(heights), np.asarray(areas)


def main() -> None:
    with rasterio.open(BUILD_DIR / "ndsm_1m.tif") as src:
        chm_all = src.read(1)
        transform = src.transform
    with rasterio.open(BUILD_DIR / "dtm_1m.vrt") as dtm:
        ground_all = dtm.read(1, masked=True).filled(np.nan)
    chm_all[building_mask(chm_all.shape, transform)] = 0

    rows_total, cols_total = chm_all.shape
    trees = []
    for r0 in range(0, rows_total, TILE):
        for c0 in range(0, cols_total, TILE):
            win = Window.from_slices((max(r0 - OVERLAP, 0), min(r0 + TILE + OVERLAP, rows_total)),
                                     (max(c0 - OVERLAP, 0), min(c0 + TILE + OVERLAP, cols_total)))
            ro, co = int(win.row_off), int(win.col_off)
            chm = chm_all[ro:ro + int(win.height), co:co + int(win.width)]
            tops, heights, areas = detect(chm)
            for (r, c), h, a in zip(tops, heights, areas):
                gr, gc = r + ro, c + co
                if not (r0 <= gr < r0 + TILE and c0 <= gc < c0 + TILE) or a < MIN_CROWN_AREA or h > MAX_HEIGHT:
                    continue
                x, y = transform * (gc + 0.5, gr + 0.5)
                trees.append((round(x, 1), round(y, 1), round(float(ground_all[gr, gc]), 1),
                              round(float(h), 1), round(float(np.sqrt(a / np.pi)), 1)))
        print(f"  rows {r0}–{min(r0 + TILE, rows_total)}: {len(trees)} trees so far")

    path = BUILD_DIR / "trees_lidar.csv"
    with path.open("w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["x", "y", "ground", "height", "crown_r"])
        w.writerows(trees)
    h = np.array([t[3] for t in trees])
    r = np.array([t[4] for t in trees])
    print(f"Wrote {path}: {len(trees)} trees, height median {np.median(h):.1f} m (max {h.max():.1f}), "
          f"crown radius median {np.median(r):.1f} m")


if __name__ == "__main__":
    main()
