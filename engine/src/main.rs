//! heathmap 3D engine prototype.
//!
//! Native builds also start the Bevy Remote Protocol on port 15702 (via
//! bevy_brp_extras), so tools can inspect the world, send input and take
//! screenshots. Everything else compiles for wasm32 as well.

mod avatar;
mod budget;
mod buildings;
mod camera;
mod ground;
mod import;
mod landmarks;
mod level;
mod lines;
mod lod;
mod pick;
mod props;
mod runs;
mod terrain;
mod textures;
mod trees;
mod ui;
mod water;
mod bookmarks;

use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::prelude::*;

use avatar::AvatarPlugin;
use budget::{BudgetPlugin, shadow_cascades};
use buildings::BuildingsPlugin;
use ground::GroundPlugin;
use import::ImportPlugin;
use camera::CameraPlugin;
use landmarks::LandmarksPlugin;
use level::LevelPlugin;
use lines::LinesPlugin;
use lod::LodPlugin;
use pick::PickPlugin;
use props::PropsPlugin;
use runs::RunsPlugin;
use terrain::TerrainPlugin;
use trees::TreesPlugin;
use ui::{Sun, UiPlugin};
use water::WaterPlugin;
use bookmarks::BookmarksPlugin;

fn main() {
    // In the browser, show panic messages in the console (otherwise only "unreachable" appears).
    #[cfg(target_arch = "wasm32")]
    std::panic::set_hook(Box::new(|info| web_sys::console::error_1(&info.to_string().into())));

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(AssetPlugin {
        // There are no .meta files; without this the browser requests one per asset (each a 404).
        meta_check: bevy::asset::AssetMetaCheck::Never,
        ..default()
    }).set(WindowPlugin {
        primary_window: Some(Window {
            title: "heathmap".into(),
            canvas: Some("#heathmap".into()),
            fit_canvas_to_parent: true,
            ..default()
        }),
        ..default()
    }))
    .add_plugins(FrameTimeDiagnosticsPlugin::default())
    .add_plugins((
        LevelPlugin { dir: "levels/heath" },
        LodPlugin,
        BudgetPlugin,
        GroundPlugin,
        TerrainPlugin,
        TreesPlugin,
        BuildingsPlugin,
        LinesPlugin,
        PickPlugin,
        CameraPlugin,
        UiPlugin,
    ))
    // Bevy plugin tuples hold at most 15 entries, so the world content goes in a second group.
    .add_plugins((
        WaterPlugin,
        PropsPlugin,
        LandmarksPlugin,
        AvatarPlugin,
        RunsPlugin,
        ImportPlugin,
        BookmarksPlugin,
    ))
    // Out of focus (another window, or the page's tab in the background) redraw at 5 Hz
    // instead of 60: rendering flat out in an unattended window kept a laptop at ~95 °C.
    .insert_resource(bevy::winit::WinitSettings {
        focused_mode: bevy::winit::UpdateMode::Continuous,
        unfocused_mode: bevy::winit::UpdateMode::reactive_low_power(std::time::Duration::from_millis(200)),
    })
    .insert_resource(ClearColor(Color::srgb(0.78, 0.85, 0.92)))
    .insert_resource(GlobalAmbientLight { brightness: 450.0, ..default() })
    .add_systems(Startup, spawn_sun);

    #[cfg(not(target_arch = "wasm32"))]
    app.add_plugins(bevy_brp_extras::BrpExtrasPlugin);

    // HEATHMAP_PROFILE=1 logs CPU and GPU time per render pass every few seconds.
    if std::env::var_os("HEATHMAP_PROFILE").is_some() {
        app.add_plugins((
            bevy::render::diagnostic::RenderDiagnosticsPlugin,
            bevy::diagnostic::LogDiagnosticsPlugin { wait_duration: std::time::Duration::from_secs(3), ..default() },
        ));
    }

    app.run();
}

/// Sunlight casting shadows from terrain, trees and buildings; its direction comes from the panel.
fn spawn_sun(mut commands: Commands) {
    commands.spawn((
        Sun,
        DirectionalLight { illuminance: 9_000.0, shadow_maps_enabled: true, ..default() },
        Transform::default(),
        shadow_cascades(1000.0),
    ));
}
