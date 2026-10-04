# heathmap

A map of Hampstead Heath, London, in two versions:

| Version | Live |
|---|---|
| **v1 — 3D engine** | https://maxwellhays.github.io/heathmap/3d/ |
| **v0 — web map** | https://maxwellhays.github.io/heathmap/ |

- **v1 — 3D engine** (`engine/`, current focus): a 3D renderer built with [Bevy](https://bevyengine.org) 0.19.
  LIDAR terrain, trees, buildings, paths, landmarks and street furniture; walk, run or fly around the
  Heath, see your own runs as routes and race ghost runners of them. Runs natively and in the browser
  (WebAssembly): [open the 3D version](https://maxwellhays.github.io/heathmap/3d/).
- **v0 — web map** (`web/`): an interactive topographic map built with MapLibre GL — contours, footpaths
  by surface, woodland and meadows, named hills, landmarks. Supports a tilted view and 3D terrain.
  [Open the web map](https://maxwellhays.github.io/heathmap/).

## Layout

- `data/` — Python pipeline that downloads source data and generates the layers for both versions
- `engine/` — v1, the Bevy app (see [engine/README.md](engine/README.md) for views, controls and remote control)
- `web/` — v0, the site (Vite + TypeScript + MapLibre GL); all visual settings live in `web/src/config.ts`

Generated data (`web/public/data/`, `engine/assets/levels/`) is not committed; it is built locally
(and for the site, in CI) by the data pipeline.

## Running v1 (the Bevy engine)

Requirements: [GDAL](https://gdal.org) command-line tools, [uv](https://docs.astral.sh/uv/), a Rust toolchain.

```bash
data/build.sh                          # download sources (cached in data/raw/) and generate map layers
(cd data && uv run export_engine.py)   # write the engine's level to engine/assets/levels/heath/

cd engine
cargo run
```

The first build compiles Bevy and takes a while. Dependencies are optimised even in dev builds, and debug
info is trimmed so the link step stays small; `engine/.cargo/config.toml` limits the build to 6 parallel
jobs. On a machine with little memory, cap the build so it can't freeze the system:

```bash
systemd-run --user --scope -q -p MemoryMax=10G -p MemorySwapMax=2G cargo run
```

Once built, the app can also be started directly from `engine/`; outside cargo, Bevy looks for
`assets/` next to the executable, so point it at the folder: `BEVY_ASSET_ROOT=. ./target/debug/heathmap-engine`.

Controls in short: `1`–`4` switch between map, walk, third-person and fly views; in the map view WASD
glides, Q/E turn and R/F tilt; `B` and `T` toggle buildings and trees; `Esc` releases the mouse. Native builds also serve the Bevy Remote Protocol on
`http://localhost:15702`. Details in [engine/README.md](engine/README.md).

### Your runs

The engine shows your runs as routes and ghost runners, and uses your average pace as the walking speed.
Runs are personal data and are never part of the repo, the level data or the site. You import them in
the app: **Import runs…** in the panel, or drop files on the window (or pass them on the command line).
It reads:

- your whole Strava export: the `.zip` from Strava → Settings → My Account → Download your data
  (only activities that are runs are used, using its `activities.csv`)
- single `.gpx`, `.tcx` or `.fit` files, gzipped or not
- the older `strava_runs.json`

Only runs that pass through the map are kept, on this device only: natively in
`~/.local/share/heathmap/runs.bin` (the platform's data folder), in the browser in IndexedDB.
**Forget my runs** deletes them. Dropping files on the window doesn't work natively under Wayland (a
winit limitation); use the Import button there.

### Web build

```bash
cd engine
./build-web.sh                     # writes engine/dist/
python3 -m http.server -d dist 8080
```

`dist/` holds two builds of the engine, for WebGPU and for WebGL2, and a loader page that runs
the WebGPU one where the browser supports it (add `?webgl2` to the URL to force the other). The
panel shows which one is running. Both use the `web` cargo profile, an optimised build without LTO
to keep compile time and memory reasonable. For quick iteration on one build,
`trunk serve --cargo-profile web` serves the WebGL2 version from `engine/index.html`.

## Running v0 (the web map)

Requirements: GDAL, uv, Node.js 22+.

```bash
data/build.sh            # download sources (cached in data/raw/) and generate web/public/data/
data/build.sh --refresh  # same, but re-download the OpenStreetMap extract

cd web
npm install
npm run dev              # http://localhost:5173/heathmap/
```

Both live versions are deployed from `main` by `.github/workflows/deploy.yml` (v0 at the site root,
v1 under `/3d/`).

## Data sources

- Elevation: [Environment Agency LIDAR Composite DTM 1m](https://environment.data.gov.uk/dataset/13787b9a-26a4-4775-8523-806d13af58fc), Open Government Licence v3 — bare-earth terrain (buildings and vegetation removed)
- Surface model (v1, building heights): Environment Agency LIDAR Composite First Return DSM 1m, Open Government Licence v3
- Trees (v1): Forest Research National Trees Outside Woodland map and GLA London Public Realm Trees, Open Government Licence v3
- Paths, land cover, water, parks, amenities, building footprints, street furniture: © [OpenStreetMap](https://www.openstreetmap.org/copyright) contributors, ODbL
- Places: curated in `data/places.json`, with locations from OpenStreetMap
- Basemap (v0): [OpenFreeMap](https://openfreemap.org)

## License

MIT
