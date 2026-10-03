# heathmap engine (prototype)

3D renderer for heathmap, built with [Bevy](https://bevyengine.org) 0.19. Runs natively for
development and compiles to WebAssembly for the site.

## Running

```bash
data/build.sh                 # map data pipeline (LIDAR, OSM, trees, buildings)
cd data && uv run export_engine.py   # writes engine/assets/levels/heath/
cd ../engine && cargo run
```

Controls: `1` orbit (drag pans, right or Ctrl/Shift + drag rotates and tilts, scroll zooms),
`2` walk (WASD, Shift runs, mouse looks), `3` fly (WASD, Space/C up/down, scroll sets speed),
`Esc` back to orbit, `B` buildings on/off, `T` trees on/off.

## Remote control (native builds)

Native builds start the [Bevy Remote Protocol](https://docs.rs/bevy_remote) with
[bevy_brp_extras](https://crates.io/crates/bevy_brp_extras) on `http://localhost:15702`
(JSON-RPC): inspect and edit the world, send keyboard/mouse input, take screenshots,
read frame-rate diagnostics. For example:

```bash
curl -s localhost:15702 -d '{"jsonrpc":"2.0","id":1,"method":"brp_extras/screenshot","params":{"path":"/tmp/shot.png"}}'
```

[bevy_brp_mcp](https://crates.io/crates/bevy_brp_mcp) exposes the same as an MCP server.
