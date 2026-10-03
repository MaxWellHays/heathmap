"""Turn the curated places.json into places.geojson, adding elevations from the DTM.

Places with `snapToSummit` (metres) are moved to the highest DTM cell within that
radius, so hill markers sit on the actual summit even if the source point is rough.
"""

import json

import numpy as np
import rasterio
from pyproj import Transformer
from rasterio.windows import from_bounds

from area import BUILD_DIR, OUT_DIR, ROOT

SMOOTHED_DTM = BUILD_DIR / "dtm_3m.tif"  # from build_contours.py; less noisy than 1m
to_bng = Transformer.from_crs("EPSG:4326", "EPSG:27700", always_xy=True)
to_wgs = Transformer.from_crs("EPSG:27700", "EPSG:4326", always_xy=True)


def summit(dtm, x: float, y: float, radius: float) -> tuple[float, float, float]:
    """Highest cell within `radius` metres of (x, y), as (x, y, elevation)."""
    window = from_bounds(x - radius, y - radius, x + radius, y + radius, dtm.transform).round_offsets().round_lengths()
    data = dtm.read(1, window=window, masked=True)
    rows, cols = np.indices(data.shape)
    xs, ys = rasterio.transform.xy(dtm.window_transform(window), rows, cols)
    xs, ys = np.asarray(xs).reshape(data.shape), np.asarray(ys).reshape(data.shape)
    data = np.ma.masked_where((xs - x) ** 2 + (ys - y) ** 2 > radius**2, data)
    r, c = np.unravel_index(np.ma.argmax(data), data.shape)
    return float(xs[r, c]), float(ys[r, c]), float(data[r, c])


def main() -> None:
    places = json.loads((ROOT / "places.json").read_text())
    features = []
    with rasterio.open(SMOOTHED_DTM) as dtm:
        for p in places:
            x, y = to_bng.transform(*p["lonlat"])
            if radius := p.get("snapToSummit"):
                x, y, ele = summit(dtm, x, y, radius)
            else:
                ele = float(next(dtm.sample([(x, y)]))[0])
            lon, lat = to_wgs.transform(x, y)
            props = {k: v for k, v in p.items() if k not in ("lonlat", "snapToSummit")}
            props["ele"] = round(ele)
            features.append({"type": "Feature", "properties": props,
                             "geometry": {"type": "Point", "coordinates": [round(lon, 6), round(lat, 6)]}})
            print(f"  {p['name']:32} {props['ele']:>4} m")
    out = OUT_DIR / "places.geojson"
    out.write_text(json.dumps({"type": "FeatureCollection", "features": features}, ensure_ascii=False, indent=1))
    print(f"Wrote {out}")


if __name__ == "__main__":
    main()
