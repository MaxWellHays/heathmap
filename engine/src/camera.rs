//! Camera modes (keys 1–4, Esc returns to the map):
//! 1 Map (orbit): left drag grabs the 3D point under the cursor (terrain, tree or
//!   building) and keeps it under the cursor; right drag or Ctrl/Shift + left drag
//!   orbits around that point (sideways rotates, up/down tilts); scroll zooms
//!   towards the point under the cursor.
//! 2 Walk: first person at eye height; WASD, Shift ×4.5, mouse looks.
//! 3 Third person: camera follows the player avatar; WASD moves relative to the
//!   view, mouse orbits the camera around the player.
//! 4 Fly: free flight; WASD, Space/C up/down, Shift ×4.5, scroll sets speed.
//!
//! Walk and third person both move the `Player` (the future runner / game avatar).

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::buildings::BuildingIndex;
use crate::level::{Heightmap, Level, LevelState};
use crate::pick::{PickGrid, raycast, terrain_hit};

const EYE_HEIGHT: f32 = 1.7;
const WALK_SPEED: f32 = 1.4; // m/s
const SHIFT_MULTIPLIER: f32 = 4.5;
const LOOK_SENSITIVITY: f32 = 0.0025; // radians per pixel
const ORBIT_SENSITIVITY: f32 = 0.005;
const MIN_TILT: f32 = 0.08; // map view: radians below the horizon (almost level)
const MAX_TILT: f32 = std::f32::consts::FRAC_PI_2 - 0.001; // straight down
const FOLLOW_DISTANCE: f32 = 5.0;
const MIN_CAMERA_CLEARANCE: f32 = 2.0; // map view: metres above the terrain

#[derive(States, Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CameraMode {
    #[default]
    Map,
    Walk,
    ThirdPerson,
    Fly,
}

/// The avatar walked in walk / third-person modes.
#[derive(Component)]
pub struct Player {
    /// Facing direction, radians (0 = north).
    pub heading: f32,
}

/// View angles for the first-person, third-person and fly cameras.
#[derive(Component)]
pub struct ViewAngles {
    pub yaw: f32,
    pub pitch: f32,
    pub fly_speed: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum DragKind {
    Pan,
    Orbit,
}

/// Where the map view should look when it is entered next (set when leaving walk/fly).
/// Applied on entering the state, because the previous mode's camera systems still run
/// in the frame the switch is requested.
#[derive(Resource, Default)]
struct MapReturn(Option<(Vec3, f32)>);

#[derive(Default)]
struct Drag {
    active: Option<(DragKind, Vec3)>,
}

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<CameraMode>()
            .init_resource::<MapReturn>()
            .add_systems(Startup, spawn_camera)
            .add_systems(OnEnter(LevelState::Ready), (place_on_level, spawn_player))
            .add_systems(Update, switch_mode.run_if(in_state(LevelState::Ready)))
            .add_systems(Update, map_controls.run_if(in_state(CameraMode::Map).and_then(in_state(LevelState::Ready))))
            .add_systems(
                Update,
                (look, walk_player.run_if(in_state(CameraMode::Walk).or_else(in_state(CameraMode::ThirdPerson))))
                    .chain()
                    .run_if(not(in_state(CameraMode::Map))),
            )
            .add_systems(Update, follow_first_person.after(walk_player).run_if(in_state(CameraMode::Walk)))
            .add_systems(Update, follow_third_person.after(walk_player).run_if(in_state(CameraMode::ThirdPerson)))
            .add_systems(Update, fly.after(look).run_if(in_state(CameraMode::Fly)))
            .add_systems(Update, avatar_visibility)
            .add_systems(OnEnter(CameraMode::Map), (release_cursor, enter_map))
            .add_systems(OnExit(CameraMode::Map), capture_cursor);
    }
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection { far: 30_000.0, near: 0.2, ..default() }),
        ViewAngles { yaw: 0.0, pitch: 0.0, fly_speed: 25.0 },
        Transform::from_xyz(0.0, 3000.0, 0.0).looking_at(Vec3::ZERO, Vec3::NEG_Z),
        DistanceFog {
            color: Color::srgb(0.78, 0.85, 0.92),
            falloff: FogFalloff::Linear { start: 3000.0, end: 12000.0 },
            ..default()
        },
    ));
}

/// Start over Parliament Hill, looking north across the Heath.
fn place_on_level(level: Res<Level>, mut cams: Query<&mut Transform, With<Camera3d>>) {
    let Ok(mut transform) = cams.single_mut() else { return };
    let (x, z) = (665.0, 366.0); // Parliament Hill summit in level coordinates
    let focus = Vec3::new(x, level.heightmap.sample(x, z), z);
    *transform = view_from(focus, 0.3, 0.75, 2200.0);
}

/// Camera looking at `focus` from `distance`, rotated `yaw` around the vertical and tilted `tilt` below the horizon.
fn view_from(focus: Vec3, yaw: f32, tilt: f32, distance: f32) -> Transform {
    let rotation = Quat::from_euler(EulerRot::YXZ, yaw, -tilt, 0.0);
    Transform::from_translation(focus + rotation * Vec3::new(0.0, 0.0, distance)).looking_at(focus, Vec3::Y)
}

fn spawn_player(
    mut commands: Commands,
    level: Res<Level>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let (x, z) = (665.0, 366.0);
    let body = materials.add(StandardMaterial { base_color: Color::srgb(0.95, 0.45, 0.10), ..default() });
    let skin = materials.add(StandardMaterial { base_color: Color::srgb(0.85, 0.68, 0.55), ..default() });
    commands
        .spawn((
            Player { heading: 0.0 },
            Name::new("Player"),
            Transform::from_xyz(x, level.heightmap.sample(x, z), z),
            Visibility::Hidden,
        ))
        .with_children(|p| {
            p.spawn((Mesh3d(meshes.add(Capsule3d::new(0.25, 0.9))), MeshMaterial3d(body), Transform::from_xyz(0.0, 0.95, 0.0)));
            p.spawn((Mesh3d(meshes.add(Sphere::new(0.13))), MeshMaterial3d(skin), Transform::from_xyz(0.0, 1.62, 0.0)));
        });
}

fn cursor_ray(window: &Window, camera: &Camera, cam_tf: &GlobalTransform) -> Option<Ray3d> {
    camera.viewport_to_world(cam_tf, window.cursor_position()?).ok()
}

fn tilt_of(transform: &Transform) -> f32 {
    (-transform.forward().y).clamp(-1.0, 1.0).asin()
}

#[allow(clippy::too_many_arguments)]
fn map_controls(
    window: Query<&Window, With<PrimaryWindow>>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    level: Res<Level>,
    index: Res<BuildingIndex>,
    grid: Res<PickGrid>,
    mut cams: Query<(&Camera, &GlobalTransform, &mut Transform)>,
    mut drag: Local<Drag>,
) {
    let (Ok(window), Ok((camera, cam_gtf, mut transform))) = (window.single(), cams.single_mut()) else { return };
    let hm = &level.heightmap;
    let modifier = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let pick = || cursor_ray(window, camera, cam_gtf).and_then(|ray| raycast(ray, &level, &index, &grid));

    // Start a drag: remember the 3D point under the cursor.
    let start = if buttons.just_pressed(MouseButton::Right) || (buttons.just_pressed(MouseButton::Left) && modifier) {
        Some(DragKind::Orbit)
    } else if buttons.just_pressed(MouseButton::Left) {
        Some(DragKind::Pan)
    } else {
        None
    };
    if let Some(kind) = start {
        drag.active = pick().map(|p| (kind, p));
    }
    if !buttons.any_pressed([MouseButton::Left, MouseButton::Right]) {
        drag.active = None;
    }

    match drag.active {
        Some((DragKind::Pan, anchor)) => {
            // Move the camera so the ray under the cursor passes through the anchor again,
            // intersecting the horizontal plane through the anchor.
            if let Some(ray) = cursor_ray(window, camera, cam_gtf)
                && ray.direction.y < -0.01
            {
                let t = (anchor.y - ray.origin.y) / ray.direction.y;
                let under_cursor = ray.get_point(t);
                transform.translation += (anchor - under_cursor).with_y(0.0);
            }
        }
        Some((DragKind::Orbit, anchor)) if motion.delta != Vec2::ZERO => {
            let yaw = Quat::from_rotation_y(-motion.delta.x * ORBIT_SENSITIVITY);
            let tilt = tilt_of(&transform);
            let new_tilt = (tilt + motion.delta.y * ORBIT_SENSITIVITY).clamp(MIN_TILT, MAX_TILT);
            let pitch = Quat::from_axis_angle(*transform.right(), -(new_tilt - tilt));
            let rotation = yaw * pitch;
            transform.translation = anchor + rotation * (transform.translation - anchor);
            transform.rotation = rotation * transform.rotation;
        }
        _ => {}
    }

    let lines = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / 40.0,
    };
    if lines != 0.0
        && let Some(target) = pick()
    {
        let factor = 0.85f32.powf(lines);
        let offset = transform.translation - target;
        let distance = (offset.length() * factor).clamp(3.0, 15_000.0);
        transform.translation = target + offset.normalize() * distance;
    }

    // Never go underground.
    let p = transform.translation;
    let floor = hm.sample(p.x, p.z) + MIN_CAMERA_CLEARANCE;
    if p.y < floor {
        transform.translation.y = floor;
    }
}

fn switch_mode(
    keys: Res<ButtonInput<KeyCode>>,
    mode: Res<State<CameraMode>>,
    mut next: ResMut<NextState<CameraMode>>,
    hm: Res<Heightmap>,
    mut cams: Query<(&Camera, &GlobalTransform, &mut ViewAngles, &mut Transform), Without<Player>>,
    mut players: Query<(&mut Player, &mut Transform), Without<Camera3d>>,
    mut map_return: ResMut<MapReturn>,
) {
    let target = if keys.just_pressed(KeyCode::Digit1) || keys.just_pressed(KeyCode::Escape) {
        CameraMode::Map
    } else if keys.just_pressed(KeyCode::Digit2) {
        CameraMode::Walk
    } else if keys.just_pressed(KeyCode::Digit3) {
        CameraMode::ThirdPerson
    } else if keys.just_pressed(KeyCode::Digit4) {
        CameraMode::Fly
    } else {
        return;
    };
    let current = *mode.get();
    if target == current {
        return;
    }
    let (Ok((camera, cam_gtf, mut angles, mut cam)), Ok((mut player, mut player_tf))) = (cams.single_mut(), players.single_mut())
    else {
        return;
    };
    let (yaw, _, _) = cam.rotation.to_euler(EulerRot::YXZ);
    match (current, target) {
        (CameraMode::Map, CameraMode::Walk | CameraMode::ThirdPerson) => {
            // Drop the player where the centre of the screen points.
            let centre = camera.logical_viewport_size().map(|s| s / 2.0);
            let hit = centre
                .and_then(|c| camera.viewport_to_world(cam_gtf, c).ok())
                .and_then(|ray| terrain_hit(ray, &hm, 30_000.0).map(|t| ray.get_point(t)));
            if let Some(p) = hit {
                player_tf.translation = p;
            }
            player.heading = yaw;
            angles.yaw = yaw;
            angles.pitch = if target == CameraMode::Walk { 0.0 } else { -0.3 };
        }
        (CameraMode::Map, CameraMode::Fly) => {
            angles.yaw = yaw;
            angles.pitch = -tilt_of(&cam) * 0.5;
        }
        (_, CameraMode::Map) => {
            // Look down at where we were from a few hundred metres away.
            let focus = if current == CameraMode::Fly {
                let p = cam.translation;
                Vec3::new(p.x, hm.sample(p.x, p.z), p.z)
            } else {
                player_tf.translation
            };
            map_return.0 = Some((focus, angles.yaw));
        }
        (CameraMode::Fly, CameraMode::Walk | CameraMode::ThirdPerson) => {
            let p = cam.translation;
            player_tf.translation = Vec3::new(p.x, hm.sample(p.x, p.z), p.z);
            player.heading = angles.yaw;
        }
        (CameraMode::Walk | CameraMode::ThirdPerson, CameraMode::Fly) => {
            cam.translation = player_tf.translation + Vec3::Y * 30.0;
        }
        _ => {}
    }
    next.set(target);
}

fn enter_map(mut map_return: ResMut<MapReturn>, mut cams: Query<&mut Transform, With<Camera3d>>) {
    if let (Some((focus, yaw)), Ok(mut cam)) = (map_return.0.take(), cams.single_mut()) {
        *cam = view_from(focus, yaw, 0.8, 350.0);
    }
}

fn look(motion: Res<AccumulatedMouseMotion>, mode: Res<State<CameraMode>>, mut cams: Query<(&mut ViewAngles, &mut Transform)>) {
    let Ok((mut angles, mut transform)) = cams.single_mut() else { return };
    angles.yaw -= motion.delta.x * LOOK_SENSITIVITY;
    angles.pitch = (angles.pitch - motion.delta.y * LOOK_SENSITIVITY).clamp(-1.5, 1.5);
    if *mode.get() != CameraMode::ThirdPerson {
        transform.rotation = Quat::from_euler(EulerRot::YXZ, angles.yaw, angles.pitch, 0.0);
    }
}

fn move_input(keys: &ButtonInput<KeyCode>) -> Vec3 {
    let mut v = Vec3::ZERO;
    if keys.pressed(KeyCode::KeyW) { v.z -= 1.0; }
    if keys.pressed(KeyCode::KeyS) { v.z += 1.0; }
    if keys.pressed(KeyCode::KeyA) { v.x -= 1.0; }
    if keys.pressed(KeyCode::KeyD) { v.x += 1.0; }
    v.normalize_or_zero()
}

fn boost(keys: &ButtonInput<KeyCode>) -> f32 {
    if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) { SHIFT_MULTIPLIER } else { 1.0 }
}

/// Moves the player relative to the view direction; in third person the avatar turns to face where it walks.
fn walk_player(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    hm: Res<Heightmap>,
    cams: Query<&ViewAngles>,
    mut players: Query<(&mut Player, &mut Transform)>,
) {
    let (Ok(angles), Ok((mut player, mut transform))) = (cams.single(), players.single_mut()) else { return };
    let input = move_input(&keys);
    let step = Quat::from_rotation_y(angles.yaw) * input * WALK_SPEED * boost(&keys) * time.delta_secs();
    if step != Vec3::ZERO {
        // Turn smoothly towards the walking direction.
        let target = (-step.x).atan2(-step.z);
        let diff = (target - player.heading + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        player.heading += diff * (10.0 * time.delta_secs()).min(1.0);
    }
    let mut p = transform.translation + step;
    p.y = hm.sample(p.x, p.z);
    transform.translation = p;
    transform.rotation = Quat::from_rotation_y(player.heading);
}

fn follow_first_person(players: Query<&Transform, With<Player>>, mut cams: Query<&mut Transform, (With<Camera3d>, Without<Player>)>) {
    let (Ok(player), Ok(mut cam)) = (players.single(), cams.single_mut()) else { return };
    cam.translation = player.translation + Vec3::Y * EYE_HEIGHT;
}

fn follow_third_person(
    hm: Res<Heightmap>,
    players: Query<&Transform, With<Player>>,
    mut cams: Query<(&ViewAngles, &mut Transform), (With<Camera3d>, Without<Player>)>,
) {
    let (Ok(player), Ok((angles, mut cam))) = (players.single(), cams.single_mut()) else { return };
    let head = player.translation + Vec3::Y * 1.5;
    let rotation = Quat::from_euler(EulerRot::YXZ, angles.yaw, angles.pitch.min(0.4), 0.0);
    let mut eye = head + rotation * Vec3::new(0.0, 0.0, FOLLOW_DISTANCE);
    eye.y = eye.y.max(hm.sample(eye.x, eye.z) + 0.5);
    *cam = Transform::from_translation(eye).looking_at(head, Vec3::Y);
}

fn fly(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    scroll: Res<AccumulatedMouseScroll>,
    hm: Res<Heightmap>,
    mut cams: Query<(&mut ViewAngles, &mut Transform)>,
) {
    let Ok((mut angles, mut transform)) = cams.single_mut() else { return };
    if scroll.delta.y != 0.0 {
        angles.fly_speed = (angles.fly_speed * 1.15f32.powf(scroll.delta.y.signum())).clamp(2.0, 400.0);
    }
    let mut v = transform.rotation * move_input(&keys);
    if keys.pressed(KeyCode::Space) { v.y += 1.0; }
    if keys.pressed(KeyCode::KeyC) { v.y -= 1.0; }
    let mut p = transform.translation + v * angles.fly_speed * boost(&keys) * time.delta_secs();
    p.y = p.y.max(hm.sample(p.x, p.z) + 1.0);
    transform.translation = p;
}

/// The avatar is visible except when looking through its eyes.
fn avatar_visibility(mode: Res<State<CameraMode>>, mut players: Query<&mut Visibility, With<Player>>) {
    if !mode.is_changed() {
        return;
    }
    for mut v in &mut players {
        *v = if *mode.get() == CameraMode::Walk { Visibility::Hidden } else { Visibility::Inherited };
    }
}

fn capture_cursor(mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>) {
    if let Ok(mut c) = cursor.single_mut() {
        c.grab_mode = CursorGrabMode::Locked;
        c.visible = false;
    }
}

fn release_cursor(mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>) {
    if let Ok(mut c) = cursor.single_mut() {
        c.grab_mode = CursorGrabMode::None;
        c.visible = true;
    }
}
