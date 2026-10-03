# heathmap engine (prototype)

3D renderer for heathmap, built with [Bevy](https://bevyengine.org) 0.19. Runs natively for
development and compiles to WebAssembly for the site.

## Running

```bash
data/build.sh                 # map data pipeline (LIDAR, OSM, trees, buildings)
cd data && uv run export_engine.py   # writes engine/assets/levels/heath/
cd ../engine && cargo run
```

Views (keys `1`–`4` or the panel; switching animates the camera):
- `1` Map: drag moves the point you grabbed (terrain, tree or building); right drag or
  Ctrl/Shift + drag orbits around it; scroll zooms smoothly towards the cursor
- `2` Walk: WASD, Shift runs (×4.5), mouse looks
- `3` Third person: an avatar with a follow camera; WASD, Shift, mouse orbits the camera
- `4` Fly: WASD, Space/C up/down, Shift ×4.5, scroll sets speed

In walk, third-person and fly views the mouse is captured: `Esc` releases it, clicking the
view captures it again. The panel toggles buildings (`B`), trees (`T`), roads and paths and
shadows, and sets the sun's direction and height.

## Remote control (native builds)

Native builds start the [Bevy Remote Protocol](https://docs.rs/bevy_remote) with
[bevy_brp_extras](https://crates.io/crates/bevy_brp_extras) on `http://localhost:15702`
(JSON-RPC): inspect and edit the world, send keyboard/mouse input, take screenshots,
read frame-rate diagnostics. For example:

```bash
curl -s localhost:15702 -d '{"jsonrpc":"2.0","id":1,"method":"brp_extras/screenshot","params":{"path":"/tmp/shot.png"}}'
```

[bevy_brp_mcp](https://crates.io/crates/bevy_brp_mcp) exposes the same as an MCP server.
