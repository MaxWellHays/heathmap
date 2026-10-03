//! Camera modes (keys 1–4 or the panel):
//! 1 Map (orbit): left drag grabs the 3D point under the cursor (terrain, tree or
//!   building) and keeps it under the cursor; right drag or Ctrl/Shift + left drag
//!   orbits around that point (sideways rotates, up/down tilts); scroll zooms
//!   smoothly towards the point under the cursor.
//! 2 Walk: first person at eye height; WASD, Shift ×4.5, Space jumps, mouse looks.
//! 3 Third person: camera follows the player avatar; WASD moves relative to the
//!   view, Space jumps, mouse orbits the camera around the player.
//! 4 Fly: free flight; WASD, Space/C up/down, Shift ×4.5, scroll sets speed.
//!
//! Switching modes animates the camera between views. In modes that capture the
//! mouse, Esc releases it and clicking the scene captures it again.
//! Walk and third person both move the `Player` (the future runner / game avatar).

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use bevy_egui::input::EguiWantsInput;

use crate::avatar::{FigureColors, Gait, spawn_figure};
use crate::buildings::BuildingIndex;
use crate::level::{Heightmap, Level, LevelState};
use crate::landmarks::{Landmarks, deck_height};
use crate::lines::BridgeDecks;
use crate::pick::{PickGrid, raycast, terrain_hit};

const EYE_HEIGHT: f32 = 1.7;
const WALK_SPEED: f32 = 2.2; // m/s (Shift: ×4.5 ≈ 10 m/s running)
const JUMP_SPEED: f32 = 4.8; // m/s upwards at take-off (about a 1 m jump)
const GRAVITY: f32 = 12.0; // m/s²; a little above real gravity for a snappier, game-like jump
const SHIFT_MULTIPLIER: f32 = 4.5;
const LOOK_SENSITIVITY: f32 = 0.0025; // radians per pixel
const ORBIT_SENSITIVITY: f32 = 0.005;
const MIN_TILT: f32 = 0.08; // map view: radians below the horizon (almost level)
const MAX_TILT: f32 = std::f32::consts::FRAC_PI_2 - 0.001; // straight down
const FOLLOW_DISTANCE: f32 = 5.0; // default third-person camera distance (scroll changes it)
const FOLLOW_RANGE: (f32, f32) = (1.5, 120.0);
const MIN_CAMERA_CLEARANCE: f32 = 2.0; // map view: metres above the terrain
const ZOOM_STEP: f32 = 0.82; // distance factor per scroll notch
const ZOOM_SMOOTHING: f32 = 14.0; // 1/s; higher settles faster
const START: Vec2 = Vec2::new(665.0, 366.0); // Parliament Hill summit in level coordinates

#[derive(States, Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CameraMode {
    #[default]
    Map,
    Walk,
    ThirdPerson,
    Fly,
}

impl CameraMode {
    pub const ALL: [CameraMode; 4] = [CameraMode::Map, CameraMode::Walk, CameraMode::ThirdPerson, CameraMode::Fly];

    pub fn label(self) -> &'static str {
        match self {
            CameraMode::Map => "Map",
            CameraMode::Walk => "Walk",
            CameraMode::ThirdPerson => "Third person",
            CameraMode::Fly => "Fly",
        }
    }

    pub fn captures_cursor(self) -> bool {
        self != CameraMode::Map
    }
}

/// Set by the UI (or keys) to ask for a mode change; handled by `switch_mode`.
#[derive(Resource, Default)]
pub struct ModeRequest(pub Option<CameraMode>);

/// The avatar walked in walk / third-person modes.
#[derive(Component)]
pub struct Player {
    /// Facing direction, radians (0 = north).
    pub heading: f32,
    /// Vertical speed while jumping or falling (m/s); 0 on the ground.
    pub vertical_speed: f32,
    pub airborne: bool,
}

/// View angles for the first-person, third-person and fly cameras.
#[derive(Component)]
pub struct ViewAngles {
    pub yaw: f32,
    pub pitch: f32,
    pub fly_speed: f32,
    /// Third-person camera distance from the player.
    pub follow_distance: f32,
}

/// An animated camera move between modes; mode controls pause while it runs.
#[derive(Resource, Default)]
pub struct CameraTween(Option<Tween>);

struct Tween {
    from: Transform,
    to: Transform,
    t: f32,
    duration: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum DragKind {
    Pan,
    Orbit,
}

#[derive(Default)]
struct MapInput {
    drag: Option<(DragKind, Vec3)>,
    /// Smooth zoom: point zoomed towards and the remaining zoom (natural log of the distance factor).
    zoom: Option<(Vec3, f32)>,
}

pub struct CameraPlugin;

/// False while a text field (e.g. a bookmark name) has keyboard focus, so typing
/// doesn't also move the camera or switch modes.
pub fn keyboard_free(egui: Option<Res<EguiWantsInput>>) -> bool {
    egui.is_none_or(|e| !e.wants_keyboard_input())
}

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        let no_tween = |t: Res<CameraTween>| t.0.is_none();
        app.init_state::<CameraMode>()
            .init_resource::<ModeRequest>()
            .init_resource::<CameraTween>()
            .add_systems(Startup, spawn_camera)
            .add_systems(OnEnter(LevelState::Ready), (place_on_level, spawn_player))
            .add_systems(Update, (mode_keys.run_if(keyboard_free), switch_mode).chain().run_if(in_state(LevelState::Ready)))
            .add_systems(
                Update,
                (
                    map_controls.run_if(in_state(CameraMode::Map)),
                    (
                        look,
                        walk_player
                            .run_if(in_state(CameraMode::Walk).or_else(in_state(CameraMode::ThirdPerson)))
                            .run_if(keyboard_free),
                    )
                        .chain()
                        .run_if(not(in_state(CameraMode::Map))),
                    follow_first_person.after(walk_player).run_if(in_state(CameraMode::Walk)),
                    follow_third_person.after(walk_player).run_if(in_state(CameraMode::ThirdPerson)),
                    fly.after(look).run_if(in_state(CameraMode::Fly)).run_if(keyboard_free),
                )
                    .after(switch_mode)
                    .run_if(in_state(LevelState::Ready).and_then(no_tween)),
            )
            .add_systems(Update, (run_tween.after(switch_mode), cursor_capture, avatar_visibility));
    }
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection { far: 30_000.0, near: 0.2, ..default() }),
        ViewAngles { yaw: 0.0, pitch: 0.0, fly_speed: 25.0, follow_distance: FOLLOW_DISTANCE },
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
    let focus = Vec3::new(START.x, level.heightmap.sample(START.x, START.y), START.y);
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
    let player = commands
        .spawn((
            Player { heading: 0.0, vertical_speed: 0.0, airborne: false },
            Gait::default(),
            Name::new("Player"),
            Transform::from_xyz(START.x, level.heightmap.sample(START.x, START.y), START.y),
            Visibility::Hidden,
        ))
        .id();
    spawn_figure(&mut commands, player, FigureColors::default(), &mut meshes, &mut materials);
}

fn cursor_ray(window: &Window, camera: &Camera, cam_tf: &GlobalTransform) -> Option<Ray3d> {
    camera.viewport_to_world(cam_tf, window.cursor_position()?).ok()
}

fn tilt_of(transform: &Transform) -> f32 {
    (-transform.forward().y).clamp(-1.0, 1.0).asin()
}

#[allow(clippy::too_many_arguments)]
fn map_controls(
    time: Res<Time>,
    window: Query<&Window, With<PrimaryWindow>>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    egui: Res<EguiWantsInput>,
    level: Res<Level>,
    index: Res<BuildingIndex>,
    grid: Res<PickGrid>,
    mut cams: Query<(&Camera, &GlobalTransform, &mut Transform)>,
    mut input: Local<MapInput>,
) {
    let (Ok(window), Ok((camera, cam_gtf, mut transform))) = (window.single(), cams.single_mut()) else { return };
    let hm = &level.heightmap;
    let over_ui = egui.wants_pointer_input() || egui.is_pointer_over_area();
    let modifier = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let pick = || cursor_ray(window, camera, cam_gtf).and_then(|ray| raycast(ray, &level, &index, &grid));

    // Start a drag (not when clicking the panel): remember the 3D point under the cursor.
    let start = if buttons.just_pressed(MouseButton::Right) || (buttons.just_pressed(MouseButton::Left) && modifier) {
        Some(DragKind::Orbit)
    } else if buttons.just_pressed(MouseButton::Left) {
        Some(DragKind::Pan)
    } else {
        None
    };
    if let Some(kind) = start.filter(|_| !over_ui) {
        input.drag = pick().map(|p| (kind, p));
        input.zoom = None;
    }
    if !buttons.any_pressed([MouseButton::Left, MouseButton::Right]) {
        input.drag = None;
    }

    match input.drag {
        Some((DragKind::Pan, anchor)) => {
            // Move the camera so the ray under the cursor passes through the anchor again,
            // intersecting the horizontal plane through the anchor.
            if let Some(ray) = cursor_ray(window, camera, cam_gtf)
                && ray.direction.y < -0.01
            {
                let t = (anchor.y - ray.origin.y) / ray.direction.y;
                transform.translation += (anchor - ray.get_point(t)).with_y(0.0);
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

    // Scrolling queues zoom towards the point under the cursor; it is applied smoothly below.
    let lines = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / 40.0,
    };
    if lines != 0.0
        && !over_ui
        && let Some(target) = pick()
    {
        let pending = input.zoom.map_or(0.0, |(_, p)| p);
        input.zoom = Some((target, pending + lines * ZOOM_STEP.ln()));
    }
    if let Some((target, pending)) = input.zoom {
        let step = pending * (1.0 - (-ZOOM_SMOOTHING * time.delta_secs()).exp());
        let offset = transform.translation - target;
        let distance = (offset.length() * step.exp()).clamp(3.0, 15_000.0);
        transform.translation = target + offset.normalize() * distance;
        let rest = pending - step;
        input.zoom = (rest.abs() > 1e-3).then_some((target, rest));
    }

    // Never go underground.
    let p = transform.translation;
    let floor = hm.sample(p.x, p.z) + MIN_CAMERA_CLEARANCE;
    if p.y < floor {
        transform.translation.y = floor;
    }
}

fn mode_keys(keys: Res<ButtonInput<KeyCode>>, mut request: ResMut<ModeRequest>) {
    for (key, mode) in [
        (KeyCode::Digit1, CameraMode::Map),
        (KeyCode::Digit2, CameraMode::Walk),
        (KeyCode::Digit3, CameraMode::ThirdPerson),
        (KeyCode::Digit4, CameraMode::Fly),
    ] {
        if keys.just_pressed(key) {
            request.0 = Some(mode);
        }
    }
}

/// Handles a mode request: places the player if needed and animates the camera to the new view.
#[allow(clippy::too_many_arguments)]
fn switch_mode(
    mut request: ResMut<ModeRequest>,
    mode: Res<State<CameraMode>>,
    mut next: ResMut<NextState<CameraMode>>,
    mut tween: ResMut<CameraTween>,
    hm: Res<Heightmap>,
    mut cams: Query<(&Camera, &GlobalTransform, &mut ViewAngles, &Transform), Without<Player>>,
    mut players: Query<(&mut Player, &mut Transform), Without<Camera3d>>,
) {
    let Some(target) = request.0.take() else { return };
    let current = *mode.get();
    if target == current || tween.0.is_some() {
        return;
    }
    let (Ok((camera, cam_gtf, mut angles, cam)), Ok((mut player, mut player_tf))) = (cams.single_mut(), players.single_mut())
    else {
        return;
    };
    let (yaw, _, _) = cam.rotation.to_euler(EulerRot::YXZ);

    // Where the player goes when entering walk / third person.
    if matches!(target, CameraMode::Walk | CameraMode::ThirdPerson) {
        match current {
            CameraMode::Map => {
                // Drop the player where the centre of the screen points.
                let centre = camera.logical_viewport_size().map(|s| s / 2.0);
                if let Some(p) = centre
                    .and_then(|c| camera.viewport_to_world(cam_gtf, c).ok())
                    .and_then(|ray| terrain_hit(ray, &hm, 30_000.0).map(|t| ray.get_point(t)))
                {
                    player_tf.translation = p;
                }
                player.heading = yaw;
                angles.yaw = yaw;
            }
            CameraMode::Fly => {
                let p = cam.translation;
                player_tf.translation = Vec3::new(p.x, hm.sample(p.x, p.z), p.z);
                player.heading = angles.yaw;
            }
            _ => {}
        }
        angles.pitch = if target == CameraMode::Walk { 0.0 } else { -0.3 };
        player_tf.rotation = Quat::from_rotation_y(player.heading);
    }
    if target == CameraMode::Fly && current == CameraMode::Map {
        angles.yaw = yaw;
        angles.pitch = -tilt_of(cam) * 0.5;
    }

    let to = match target {
        CameraMode::Walk => Transform::from_translation(player_tf.translation + Vec3::Y * EYE_HEIGHT)
            .with_rotation(Quat::from_euler(EulerRot::YXZ, angles.yaw, angles.pitch, 0.0)),
        CameraMode::ThirdPerson => third_person_view(&hm, player_tf.translation, angles.yaw, angles.pitch, angles.follow_distance),
        CameraMode::Fly => {
            let lift = if current == CameraMode::Map { 0.0 } else { 30.0 };
            Transform::from_translation(cam.translation + Vec3::Y * lift)
                .with_rotation(Quat::from_euler(EulerRot::YXZ, angles.yaw, angles.pitch, 0.0))
        }
        CameraMode::Map => {
            // Look down at where we were from a few hundred metres away.
            let focus = if current == CameraMode::Fly {
                let p = cam.translation;
                Vec3::new(p.x, hm.sample(p.x, p.z), p.z)
            } else {
                player_tf.translation
            };
            view_from(focus, angles.yaw, 0.8, 350.0)
        }
    };
    // Longer moves take a little longer, within 0.6–2 s.
    let duration = (0.6 + cam.translation.distance(to.translation) / 1500.0).min(2.0);
    tween.0 = Some(Tween { from: *cam, to, t: 0.0, duration });
    next.set(target);
}

fn run_tween(
    time: Res<Time>,
    mut tween: ResMut<CameraTween>,
    hm: Option<Res<Heightmap>>,
    mut cams: Query<&mut Transform, With<Camera3d>>,
) {
    let (Some(tw), Ok(mut cam), Some(hm)) = (tween.0.as_mut(), cams.single_mut(), hm) else { return };
    tw.t = (tw.t + time.delta_secs() / tw.duration).min(1.0);
    let s = tw.t * tw.t * (3.0 - 2.0 * tw.t); // ease in-out
    let mut p = tw.from.translation.lerp(tw.to.translation, s);
    // Arc up a little on long moves so the camera doesn't skim through hills.
    let lift = (tw.from.translation.distance(tw.to.translation) * 0.15).min(150.0);
    p.y += lift * (std::f32::consts::PI * s).sin();
    p.y = p.y.max(hm.sample(p.x, p.z) + 1.0);
    cam.translation = p;
    cam.rotation = tw.from.rotation.slerp(tw.to.rotation, s);
    if tw.t >= 1.0 {
        tween.0 = None;
    }
}

fn look(
    motion: Res<AccumulatedMouseMotion>,
    mode: Res<State<CameraMode>>,
    cursor: Query<&CursorOptions, With<PrimaryWindow>>,
    mut cams: Query<(&mut ViewAngles, &mut Transform)>,
) {
    let Ok((mut angles, mut transform)) = cams.single_mut() else { return };
    // Mouse looks only while captured (Esc releases it to use the panel).
    if cursor.single().is_ok_and(cursor_captured) {
        angles.yaw -= motion.delta.x * LOOK_SENSITIVITY;
        angles.pitch = (angles.pitch - motion.delta.y * LOOK_SENSITIVITY).clamp(-1.5, 1.5);
    }
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

/// Moves the player relative to the view direction; the avatar turns to face where it
/// walks. Space jumps; gravity brings the player back down onto the terrain.
fn walk_player(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    hm: Res<Heightmap>,
    decks: Option<Res<BridgeDecks>>,
    landmarks: Option<Res<Landmarks>>,
    cams: Query<&ViewAngles>,
    mut players: Query<(&mut Player, &mut Gait, &mut Transform)>,
) {
    let (Ok(angles), Ok((mut player, mut gait, mut transform))) = (cams.single(), players.single_mut()) else { return };
    let step = Quat::from_rotation_y(angles.yaw) * move_input(&keys) * WALK_SPEED * boost(&keys) * time.delta_secs();
    if step != Vec3::ZERO {
        let target = (-step.x).atan2(-step.z);
        let diff = (target - player.heading + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        player.heading += diff * (10.0 * time.delta_secs()).min(1.0);
    }
    let dt = time.delta_secs();
    if keys.just_pressed(KeyCode::Space) && !player.airborne {
        player.vertical_speed = JUMP_SPEED;
        player.airborne = true;
    }
    let mut p = transform.translation + step;
    let terrain = hm.sample(p.x, p.z);
    // On a bridge deck (and not walking underneath it), the deck is the ground.
    let mut ground = match decks.as_ref().and_then(|d| d.height_at(Vec2::new(p.x, p.z))) {
        Some(deck) if deck > terrain && transform.translation.y > deck - 1.0 => deck,
        _ => terrain,
    };
    // On a landmark terrace (the pergola), walk on its deck.
    if landmarks.as_ref().is_some_and(|l| l.pergola_at(Vec2::new(p.x, p.z)).is_some()) {
        ground = ground.max(deck_height(&hm, Vec2::new(p.x, p.z)));
    }
    if player.airborne {
        player.vertical_speed -= GRAVITY * dt;
        p.y += player.vertical_speed * dt;
        if p.y <= ground {
            p.y = ground;
            player.vertical_speed = 0.0;
            player.airborne = false;
        }
    } else {
        p.y = ground;
    }
    // Animation input for the runner figure.
    let dt_safe = dt.max(1e-4);
    gait.speed = (p - transform.translation).with_y(0.0).length() / dt_safe;
    gait.airborne = player.airborne;
    transform.translation = p;
    transform.rotation = Quat::from_rotation_y(player.heading);
}

fn follow_first_person(players: Query<&Transform, With<Player>>, mut cams: Query<&mut Transform, (With<Camera3d>, Without<Player>)>) {
    let (Ok(player), Ok(mut cam)) = (players.single(), cams.single_mut()) else { return };
    cam.translation = player.translation + Vec3::Y * EYE_HEIGHT;
}

fn third_person_view(hm: &Heightmap, feet: Vec3, yaw: f32, pitch: f32, distance: f32) -> Transform {
    let head = feet + Vec3::Y * 1.5;
    let rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch.min(0.4), 0.0);
    let mut eye = head + rotation * Vec3::new(0.0, 0.0, distance);
    eye.y = eye.y.max(hm.sample(eye.x, eye.z) + 0.5);
    Transform::from_translation(eye).looking_at(head, Vec3::Y)
}

/// Third-person camera; scrolling moves it closer to or further from the player.
fn follow_third_person(
    hm: Res<Heightmap>,
    scroll: Res<AccumulatedMouseScroll>,
    egui: Option<Res<EguiWantsInput>>,
    players: Query<&Transform, With<Player>>,
    mut cams: Query<(&mut ViewAngles, &mut Transform), (With<Camera3d>, Without<Player>)>,
) {
    let (Ok(player), Ok((mut angles, mut cam))) = (players.single(), cams.single_mut()) else { return };
    let over_ui = egui.is_some_and(|e| e.is_pointer_over_area());
    if scroll.delta.y != 0.0 && !over_ui {
        let lines = match scroll.unit {
            MouseScrollUnit::Line => scroll.delta.y,
            MouseScrollUnit::Pixel => scroll.delta.y / 40.0,
        };
        angles.follow_distance = (angles.follow_distance * 0.85f32.powf(lines)).clamp(FOLLOW_RANGE.0, FOLLOW_RANGE.1);
    }
    *cam = third_person_view(&hm, player.translation, angles.yaw, angles.pitch, angles.follow_distance);
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

/// Captures the mouse in walk / third-person / fly modes. Esc releases it (to use the
/// panel or other windows); clicking the scene captures it again.
fn cursor_capture(
    mode: Res<State<CameraMode>>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    egui: Option<Res<EguiWantsInput>>,
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    let Ok(mut c) = cursor.single_mut() else { return };
    let captured = cursor_captured(&c);
    let over_ui = egui.as_ref().is_some_and(|e| e.wants_pointer_input() || e.is_pointer_over_area());
    let typing = egui.as_ref().is_some_and(|e| e.wants_keyboard_input());
    let want = if !mode.get().captures_cursor() || (keys.just_pressed(KeyCode::Escape) && !typing) {
        false
    } else if mode.is_changed() || (buttons.just_pressed(MouseButton::Left) && !over_ui) {
        true
    } else {
        captured
    };
    if want != captured {
        c.grab_mode = if want { CursorGrabMode::Locked } else { CursorGrabMode::None };
        c.visible = !want;
    }
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

/// True while the mouse is captured (for look input and UI hints).
pub fn cursor_captured(cursor: &CursorOptions) -> bool {
    cursor.grab_mode == CursorGrabMode::Locked
}
