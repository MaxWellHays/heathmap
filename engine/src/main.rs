//! heathmap 3D engine prototype.
//!
//! Native builds also start the Bevy Remote Protocol on port 15702 (via
//! bevy_brp_extras), so tools can inspect the world, send input and take
//! screenshots. Everything else compiles for wasm32 as well.

mod camera;
mod level;
mod terrain;
mod trees;

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;

use camera::{CameraMode, CameraPlugin};
use level::{Level, LevelPlugin, LevelState};
use terrain::TerrainPlugin;
use trees::{Trees, TreesPlugin};

#[derive(Component)]
struct Buildings;

#[derive(Component)]
struct Hud;

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
    .add_plugins((LevelPlugin { dir: "levels/heath" }, TerrainPlugin, TreesPlugin, CameraPlugin))
    .insert_resource(ClearColor(Color::srgb(0.78, 0.85, 0.92)))
    .insert_resource(GlobalAmbientLight { brightness: 600.0, ..default() })
    .add_systems(Startup, setup)
    .add_systems(OnEnter(LevelState::Ready), spawn_buildings)
    .add_systems(Update, (toggle_layers, update_hud));

    #[cfg(not(target_arch = "wasm32"))]
    app.add_plugins(bevy_brp_extras::BrpExtrasPlugin);

    app.run();
}

fn setup(mut commands: Commands) {
    // Afternoon sun from the south-west.
    commands.spawn((
        DirectionalLight { illuminance: 9_000.0, shadow_maps_enabled: false, ..default() },
        Transform::from_xyz(-1.0, 1.2, 1.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Hud,
        Text::new("Loading…"),
        TextFont { font_size: FontSize::Px(14.0), ..default() },
        TextColor(Color::BLACK),
        Node { position_type: PositionType::Absolute, top: px(8), left: px(10), ..default() },
    ));
}

fn spawn_buildings(mut commands: Commands, level: Res<Level>, assets: Res<AssetServer>) {
    let path = format!("{}/{}", level.dir, level.meta.buildings.file);
    commands.spawn((Buildings, Name::new("Buildings"), WorldAssetRoot(assets.load(GltfAssetLabel::Scene(0).from_asset(path)))));
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
             1 orbit | 2 walk | 3 fly | Esc back | B buildings | T trees\n\
             Orbit: drag pan | right or Ctrl+drag rotate and tilt | scroll zoom",
            mode.get()
        )
    };
}
