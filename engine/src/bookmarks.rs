//! Bookmarks: saved views (mode, camera, player position, sun) to come back to — for
//! exploring and for reporting issues at exact spots. Saved to `bookmarks.json` next
//! to the assets in native builds; the list is also readable and controllable over
//! the Bevy Remote Protocol (resource `heathmap_engine::bookmarks::Bookmarks`).

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::camera::{CameraMode, CameraTween, ModeRequest, Player, ViewAngles};
use crate::level::LevelState;
use crate::ui::LayerSettings;

#[derive(Serialize, Deserialize, Reflect, Clone, Debug)]
pub struct Bookmark {
    pub name: String,
    /// "Map", "Walk", "ThirdPerson" or "Fly".
    pub mode: String,
    pub camera_translation: Vec3,
    pub camera_rotation: Quat,
    pub player_translation: Vec3,
    pub player_heading: f32,
    pub view_yaw: f32,
    pub view_pitch: f32,
    pub sun_azimuth: f32,
    pub sun_elevation: f32,
}

#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
pub struct Bookmarks {
    pub list: Vec<Bookmark>,
    /// Set to an index to jump to that bookmark (from the panel or remotely).
    pub goto: Option<usize>,
    /// Set to a name to save the current view (from the panel or remotely).
    pub save: Option<String>,
    /// Set to an index to delete that bookmark.
    pub delete: Option<usize>,
}

pub struct BookmarksPlugin;

impl Plugin for BookmarksPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Bookmarks>()
            .insert_resource(Bookmarks { list: load(), ..default() })
            .add_systems(Update, (save_current, delete, goto).run_if(in_state(LevelState::Ready)));
    }
}

fn mode_name(mode: CameraMode) -> String {
    format!("{mode:?}")
}

fn parse_mode(name: &str) -> CameraMode {
    CameraMode::ALL.into_iter().find(|m| mode_name(*m) == name).unwrap_or(CameraMode::Map)
}

#[cfg(not(target_arch = "wasm32"))]
fn path() -> std::path::PathBuf {
    let root = std::env::var("BEVY_ASSET_ROOT").or_else(|_| std::env::var("CARGO_MANIFEST_DIR")).unwrap_or_else(|_| ".".into());
    std::path::Path::new(&root).join("bookmarks.json")
}

#[cfg(not(target_arch = "wasm32"))]
fn load() -> Vec<Bookmark> {
    std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

#[cfg(not(target_arch = "wasm32"))]
fn store(list: &[Bookmark]) {
    match serde_json::to_string_pretty(list) {
        Ok(json) => {
            if let Err(e) = std::fs::write(path(), json + "\n") {
                warn!("Could not save bookmarks: {e}");
            }
        }
        Err(e) => warn!("Could not serialise bookmarks: {e}"),
    }
}

// In the browser bookmarks live for the session only (persistence can use localStorage later).
#[cfg(target_arch = "wasm32")]
fn load() -> Vec<Bookmark> {
    Vec::new()
}

#[cfg(target_arch = "wasm32")]
fn store(_: &[Bookmark]) {}

fn save_current(
    mut bookmarks: ResMut<Bookmarks>,
    mode: Res<State<CameraMode>>,
    layers: Res<LayerSettings>,
    cams: Query<(&Transform, &ViewAngles), With<Camera3d>>,
    players: Query<(&Transform, &Player)>,
) {
    let Some(name) = bookmarks.save.take() else { return };
    let (Ok((cam, angles)), Ok((player_tf, player))) = (cams.single(), players.single()) else { return };
    let name = if name.trim().is_empty() { format!("View {}", bookmarks.list.len() + 1) } else { name };
    bookmarks.list.push(Bookmark {
        name,
        mode: mode_name(*mode.get()),
        camera_translation: cam.translation,
        camera_rotation: cam.rotation,
        player_translation: player_tf.translation,
        player_heading: player.heading,
        view_yaw: angles.yaw,
        view_pitch: angles.pitch,
        sun_azimuth: layers.sun_azimuth,
        sun_elevation: layers.sun_elevation,
    });
    store(&bookmarks.list);
}

fn delete(mut bookmarks: ResMut<Bookmarks>) {
    if let Some(i) = bookmarks.delete.take()
        && i < bookmarks.list.len()
    {
        bookmarks.list.remove(i);
        store(&bookmarks.list);
    }
}

/// Jumps straight to a bookmark (no animation, so the view is exactly as saved).
#[allow(clippy::too_many_arguments)]
fn goto(
    mut bookmarks: ResMut<Bookmarks>,
    mut next: ResMut<NextState<CameraMode>>,
    mut request: ResMut<ModeRequest>,
    mut tween: ResMut<CameraTween>,
    mut layers: ResMut<LayerSettings>,
    mut cams: Query<(&mut Transform, &mut ViewAngles), (With<Camera3d>, Without<Player>)>,
    mut players: Query<(&mut Transform, &mut Player), Without<Camera3d>>,
) {
    let Some(i) = bookmarks.goto.take() else { return };
    let Some(b) = bookmarks.list.get(i).cloned() else { return };
    let (Ok((mut cam, mut angles)), Ok((mut player_tf, mut player))) = (cams.single_mut(), players.single_mut()) else {
        return;
    };
    *tween = CameraTween::default();
    request.0 = None;
    cam.translation = b.camera_translation;
    cam.rotation = b.camera_rotation;
    // Walk and fly cameras look along their view angles, so derive those from the saved
    // rotation (robust to hand-edited bookmarks); third person keeps its own orbit angles.
    let mode = parse_mode(&b.mode);
    if matches!(mode, CameraMode::Walk | CameraMode::Fly) {
        let (yaw, pitch, _) = b.camera_rotation.to_euler(EulerRot::YXZ);
        (angles.yaw, angles.pitch) = (yaw, pitch);
    } else {
        (angles.yaw, angles.pitch) = (b.view_yaw, b.view_pitch);
    }
    player_tf.translation = b.player_translation;
    player.heading = b.player_heading;
    player_tf.rotation = Quat::from_rotation_y(b.player_heading);
    layers.sun_azimuth = b.sun_azimuth;
    layers.sun_elevation = b.sun_elevation;
    next.set(mode);
}
