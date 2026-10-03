//! heathmap 3D engine prototype.
//!
//! Native builds also start the Bevy Remote Protocol on port 15702 (via
//! bevy_brp_extras), so tools can inspect the world, send input and take
//! screenshots. Everything else compiles for wasm32 as well.

mod buildings;
mod camera;
mod level;
mod lines;
mod lod;
mod pick;
mod terrain;
mod trees;

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::light::{CascadeShadowConfig, CascadeShadowConfigBuilder, DirectionalLightShadowMap};
use bevy::prelude::*;

use buildings::{Buildings, BuildingsPlugin};
use camera::{CameraMode, CameraPlugin};
use lines::LinesPlugin;
use level::{Heightmap, LevelPlugin, LevelState};
use lod::LodPlugin;
use pick::PickPlugin;
use terrain::TerrainPlugin;
use trees::{Trees, TreesPlugin};

#[derive(Component)]
struct Hud;

#[derive(Component)]
struct Sun;

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
    .add_plugins((LevelPlugin { dir: "levels/heath" }, LodPlugin, TerrainPlugin, TreesPlugin, BuildingsPlugin, LinesPlugin, PickPlugin, CameraPlugin))
    .insert_resource(ClearColor(Color::srgb(0.78, 0.85, 0.92)))
    .insert_resource(GlobalAmbientLight { brightness: 450.0, ..default() })
    .insert_resource(DirectionalLightShadowMap { size: 4096 })
    .add_systems(Startup, setup)
    .add_systems(Update, (toggle_layers, update_hud, fit_shadows_to_view));

    #[cfg(not(target_arch = "wasm32"))]
    app.add_plugins(bevy_brp_extras::BrpExtrasPlugin);

    app.run();
}

fn setup(mut commands: Commands) {
    // Afternoon sun from the south-west, casting shadows from terrain, trees and buildings.
    commands.spawn((
        Sun,
        DirectionalLight { illuminance: 9_000.0, shadow_maps_enabled: true, ..default() },
        Transform::from_xyz(-1.0, 0.9, 1.0).looking_at(Vec3::ZERO, Vec3::Y),
        shadow_cascades(1000.0),
    ));
    commands.spawn((
        Hud,
        Text::new("Loading…"),
        TextFont { font_size: FontSize::Px(14.0), ..default() },
        TextColor(Color::BLACK),
        Node { position_type: PositionType::Absolute, top: px(8), left: px(10), ..default() },
    ));
}

/// B = buildings, T = trees.
fn toggle_layers(
    keys: Res<ButtonInput<KeyCode>>,
    mut buildings: Query<&mut Visibility, (With<Buildings>, Without<Trees>)>,
    mut trees: Query<&mut Visibility, (With<Trees>, Without<Buildings>)>,
) {
    let flip = |v: &mut Visibility| {
        *v = if *v == Visibility::Hidden { Visibility::Inherited } else { Visibility::Hidden };
    };
    if keys.just_pressed(KeyCode::KeyB) {
        buildings.iter_mut().for_each(|mut v| flip(&mut v));
    }
    if keys.just_pressed(KeyCode::KeyT) {
        trees.iter_mut().for_each(|mut v| flip(&mut v));
    }
}

fn update_hud(
    diagnostics: Res<DiagnosticsStore>,
    mode: Res<State<CameraMode>>,
    state: Res<State<LevelState>>,
    mut hud: Query<&mut Text, With<Hud>>,
) {
    let Ok(mut text) = hud.single_mut() else { return };
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|d| d.smoothed())
        .unwrap_or(0.0);
    let frame_ms = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
        .and_then(|d| d.smoothed())
        .unwrap_or(0.0);
    text.0 = if *state.get() != LevelState::Ready {
        "Loading level…".into()
    } else {
        format!(
            "{fps:.0} fps ({frame_ms:.1} ms) | mode: {:?}\n\
             1 map | 2 walk | 3 third person | 4 fly | Esc map | B buildings | T trees\n\
             Map: drag moves the grabbed point | right or Ctrl+drag orbits it | scroll zooms\n\
             Walk/fly: WASD, Shift x4.5, mouse looks; fly: Space/C up/down, scroll speed",
            mode.get()
        )
    };
}
