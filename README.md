# heathmap

An interactive topographic map of Hampstead Heath, London — contours, footpaths by surface, woodland and meadows, named hills, landmarks, and (soon) popular running routes. Supports a tilted view and 3D terrain.

Live site: https://maxwellhays.github.io/heathmap/

## Layout

- `data/` — Python pipeline that downloads source data and generates the map layers
- `web/` — the site (Vite + TypeScript + MapLibre GL); all visual settings live in `web/src/config.ts`

Generated map data (`web/public/data/`) is not committed; it is built locally and in CI by `data/build.sh`.

## Running locally

Requirements: [GDAL](https://gdal.org) command-line tools, [uv](https://docs.astral.sh/uv/), Node.js 22+.

```bash
data/build.sh            # download sources (cached in data/raw/) and generate web/public/data/
data/build.sh --refresh  # same, but re-download the OpenStreetMap extract

cd web
npm install
npm run dev              # http://localhost:5173/heathmap/
```

## Data sources

- Elevation: [Environment Agency LIDAR Composite DTM 1m](https://environment.data.gov.uk/dataset/13787b9a-26a4-4775-8523-806d13af58fc), Open Government Licence v3 — bare-earth terrain (buildings and vegetation removed)
- Paths, land cover, water, parks, amenities: © [OpenStreetMap](https://www.openstreetmap.org/copyright) contributors, ODbL
- Places: curated in `data/places.json`, with locations from OpenStreetMap
- Basemap: [OpenFreeMap](https://openfreemap.org)

## License

MIT
