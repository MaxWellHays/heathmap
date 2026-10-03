//! heathmap 3D engine prototype.
//!
//! Native builds also start the Bevy Remote Protocol on port 15702 (via
//! bevy_brp_extras), so tools can inspect the world, send input and take
//! screenshots. Everything else compiles for wasm32 as well.

mod buildings;
mod camera;
mod ground;
mod level;
mod lines;
mod lod;
mod pick;
mod terrain;
mod textures;
mod trees;
mod ui;
mod water;
mod bookmarks;

use bevy::diagnostic::FrameTimeDiagnosticsPlugin;
use bevy::light::{CascadeShadowConfig, CascadeShadowConfigBuilder, DirectionalLightShadowMap};
use bevy::prelude::*;

use buildings::BuildingsPlugin;
use ground::GroundPlugin;
use camera::CameraPlugin;
use level::{Heightmap, LevelPlugin};
use lines::LinesPlugin;
use lod::LodPlugin;
use pick::PickPlugin;
use terrain::TerrainPlugin;
use trees::TreesPlugin;
use ui::{Sun, UiPlugin};
use water::WaterPlugin;
use bookmarks::BookmarksPlugin;

fn main() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
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
        GroundPlugin,
        TerrainPlugin,
        TreesPlugin,
        BuildingsPlugin,
        LinesPlugin,
        PickPlugin,
        CameraPlugin,
        UiPlugin,
        WaterPlugin,
        BookmarksPlugin,
    ))
    .insert_resource(ClearColor(Color::srgb(0.78, 0.85, 0.92)))
    .insert_resource(GlobalAmbientLight { brightness: 450.0, ..default() })
    .insert_resource(DirectionalLightShadowMap { size: 4096 })
    .add_systems(Startup, spawn_sun)
    .add_systems(Update, fit_shadows_to_view);

    #[cfg(not(target_arch = "wasm32"))]
    app.add_plugins(bevy_brp_extras::BrpExtrasPlugin);

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

fn shadow_cascades(max_distance: f32) -> CascadeShadowConfig {
    CascadeShadowConfigBuilder {
        num_cascades: 4,
        first_cascade_far_bound: (max_distance / 12.0).max(15.0),
        maximum_distance: max_distance,
        ..default()
    }
    .build()
}

/// Shadow range follows the camera's height above the ground: close up (walking) the
/// cascades stay tight and sharp; from high above they stretch to cover the view.
fn fit_shadows_to_view(
    camera: Query<&Transform, With<Camera3d>>,
    heightmap: Option<Res<Heightmap>>,
    mut sun: Query<&mut CascadeShadowConfig, With<Sun>>,
    mut current: Local<f32>,
) {
    let (Ok(cam), Some(hm), Ok(mut cascades)) = (camera.single(), heightmap, sun.single_mut()) else { return };
    let altitude = (cam.translation.y - hm.sample(cam.translation.x, cam.translation.z)).max(1.7);
    let wanted = (altitude * 4.0).clamp(150.0, 6000.0);
    // Rebuild only on meaningful changes (cascade splits are recomputed from scratch).
    if (wanted / current.max(1.0) - 1.0).abs() > 0.15 {
        *current = wanted;
        *cascades = shadow_cascades(wanted);
    }
}
