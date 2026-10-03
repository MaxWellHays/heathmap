//! Camera modes:
//! - Orbit (map view): left drag pans, right drag or Ctrl/Shift + left drag orbits
//!   (sideways rotates, up/down tilts), scroll zooms towards the cursor.
//! - Walk: first person at eye height on the terrain; WASD to move, Shift to run,
//!   mouse to look (cursor is captured).
//! - Fly: free flight; WASD, Space/C for up/down, scroll changes speed.
//! Keys: 1 = orbit, 2 = walk, 3 = fly, Esc = back to orbit.

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::level::{Heightmap, Level, LevelState};

const EYE_HEIGHT: f32 = 1.7;
const WALK_SPEED: f32 = 1.4; // m/s
const RUN_SPEED: f32 = 4.5;
const LOOK_SENSITIVITY: f32 = 0.0025; // radians per pixel
const ORBIT_SENSITIVITY: f32 = 0.005;
const MIN_PITCH: f32 = 0.05; // orbit: radians above the horizon (camera looking almost horizontally)
const MAX_PITCH: f32 = std::f32::consts::FRAC_PI_2 - 0.001; // straight down

#[derive(States, Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CameraMode {
    #[default]
    Orbit,
    Walk,
    Fly,
}

/// Orbit camera: looks at `focus` from `distance` metres away.
#[derive(Component)]
pub struct OrbitRig {
    pub focus: Vec3,
    pub distance: f32,
    /// Rotation around the vertical axis (radians, 0 = looking north).
    pub yaw: f32,
    /// Angle below the horizon (radians, π/2 = straight down).
    pub pitch: f32,
}

/// First-person state shared by walk and fly modes.
#[derive(Component, Default)]
pub struct FirstPerson {
    pub yaw: f32,
    pub pitch: f32,
    pub fly_speed: f32,
}

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<CameraMode>()
            .add_systems(Startup, spawn_camera)
            .add_systems(OnEnter(LevelState::Ready), place_on_level)
            .add_systems(Update, switch_mode)
            .add_systems(Update, orbit_controls.run_if(in_state(CameraMode::Orbit)))
            .add_systems(
                Update,
                (look, walk.run_if(in_state(CameraMode::Walk)), fly.run_if(in_state(CameraMode::Fly)))
                    .chain()
                    .run_if(not(in_state(CameraMode::Orbit))),
            )
            .add_systems(OnEnter(CameraMode::Orbit), release_cursor)
            .add_systems(OnExit(CameraMode::Orbit), capture_cursor);
    }
}

fn spawn_camera(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection { far: 30_000.0, near: 0.2, ..default() }),
        OrbitRig { focus: Vec3::ZERO, distance: 3000.0, yaw: 0.0, pitch: 1.0 },
        FirstPerson { fly_speed: 25.0, ..default() },
        Transform::from_xyz(0.0, 3000.0, 0.0).looking_at(Vec3::ZERO, Vec3::NEG_Z),
        DistanceFog {
            color: Color::srgb(0.78, 0.85, 0.92),
            falloff: FogFalloff::Linear { start: 3000.0, end: 12000.0 },
            ..default()
        },
    ));
}

/// Start the orbit view over Parliament Hill, tilted, once terrain heights are known.
fn place_on_level(level: Res<Level>, mut rigs: Query<(&mut OrbitRig, &mut Transform)>) {
    let Ok((mut rig, mut transform)) = rigs.single_mut() else { return };
    let (x, z) = (665.0, 366.0); // Parliament Hill summit in level coordinates
    rig.focus = Vec3::new(x, level.heightmap.sample(x, z), z);
    rig.distance = 2200.0;
    rig.pitch = 0.75;
    rig.yaw = 0.3;
    *transform = orbit_transform(&rig);
}

fn orbit_transform(rig: &OrbitRig) -> Transform {
    let rotation = Quat::from_euler(EulerRot::YXZ, rig.yaw, -rig.pitch, 0.0);
    let eye = rig.focus + rotation * Vec3::new(0.0, 0.0, rig.distance);
    Transform::from_translation(eye).looking_at(rig.focus, Vec3::Y)
}

/// Point on the terrain under a screen position (ray march against the heightmap).
fn terrain_hit(camera: &Camera, cam_tf: &GlobalTransform, cursor: Vec2, hm: &Heightmap) -> Option<Vec3> {
    let ray = camera.viewport_to_world(cam_tf, cursor).ok()?;
    let mut t = 0.0;
    let mut step = 2.0;
    while t < 30_000.0 {
        let p = ray.get_point(t);
        if p.y <= hm.sample(p.x, p.z) {
            return Some(p);
        }
        t += step;
        step = (step * 1.05).min(25.0);
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn orbit_controls(
    window: Query<&Window, With<PrimaryWindow>>,
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    heightmap: Option<Res<Heightmap>>,
    mut cams: Query<(&Camera, &GlobalTransform, &mut OrbitRig, &mut Transform)>,
) {
    let Ok((camera, cam_gtf, mut rig, mut transform)) = cams.single_mut() else { return };
    let Some(hm) = heightmap else { return };
    let cursor = window.single().ok().and_then(|w| w.cursor_position());
    let delta = motion.delta;
    let modifier = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight, KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let orbiting = buttons.pressed(MouseButton::Right) || (buttons.pressed(MouseButton::Left) && modifier);

    if orbiting && delta != Vec2::ZERO {
        rig.yaw -= delta.x * ORBIT_SENSITIVITY;
        rig.pitch = (rig.pitch + delta.y * ORBIT_SENSITIVITY).clamp(MIN_PITCH, MAX_PITCH);
    } else if buttons.pressed(MouseButton::Left) && delta != Vec2::ZERO {
        // Pan: move the focus across the ground, scaled so the terrain follows the cursor.
        let right = transform.right().with_y(0.0).normalize_or_zero();
        let forward = Vec3::Y.cross(right);
        let metres_per_px = rig.distance * 0.0012;
        rig.focus += (-right * delta.x + forward * delta.y) * metres_per_px;
        rig.focus.y = hm.sample(rig.focus.x, rig.focus.z);
    }

    let lines = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / 40.0,
    };
    if lines != 0.0 {
        let factor = 0.88f32.powf(lines);
        // Zoom towards the point under the cursor: move the focus part of the way there.
        if let Some(target) = cursor.and_then(|c| terrain_hit(camera, cam_gtf, c, &hm)) {
            rig.focus = rig.focus.lerp(target, 1.0 - factor);
        }
        rig.distance = (rig.distance * factor).clamp(30.0, 12_000.0);
    }
    *transform = orbit_transform(&rig);
}

fn switch_mode(
    keys: Res<ButtonInput<KeyCode>>,
    mode: Res<State<CameraMode>>,
    mut next: ResMut<NextState<CameraMode>>,
    heightmap: Option<Res<Heightmap>>,
    mut cams: Query<(&mut FirstPerson, &mut OrbitRig, &mut Transform)>,
) {
    let target = if keys.just_pressed(KeyCode::Digit1) || keys.just_pressed(KeyCode::Escape) {
        CameraMode::Orbit
    } else if keys.just_pressed(KeyCode::Digit2) {
        CameraMode::Walk
    } else if keys.just_pressed(KeyCode::Digit3) {
        CameraMode::Fly
    } else {
        return;
    };
    if target == *mode.get() {
        return;
    }
    let (Ok((mut fp, mut rig, mut transform)), Some(hm)) = (cams.single_mut(), heightmap) else { return };
    match (mode.get(), target) {
        (CameraMode::Orbit, _) => {
            // Leave the map view facing the same way; walking starts at the orbit focus.
            fp.yaw = rig.yaw;
            fp.pitch = if target == CameraMode::Walk { 0.0 } else { -rig.pitch * 0.5 };
            if target == CameraMode::Walk {
                transform.translation = rig.focus + Vec3::Y * EYE_HEIGHT;
            }
        }
        (_, CameraMode::Orbit) => {
            // Back to the map view, focused on where we were standing.
            let p = transform.translation;
            rig.focus = Vec3::new(p.x, hm.sample(p.x, p.z), p.z);
            rig.yaw = fp.yaw;
            rig.distance = rig.distance.min(800.0);
        }
        _ => {}
    }
    next.set(target);
}

fn look(motion: Res<AccumulatedMouseMotion>, mut cams: Query<(&mut FirstPerson, &mut Transform)>) {
    let Ok((mut fp, mut transform)) = cams.single_mut() else { return };
    fp.yaw -= motion.delta.x * LOOK_SENSITIVITY;
    fp.pitch = (fp.pitch - motion.delta.y * LOOK_SENSITIVITY).clamp(-1.5, 1.5);
    transform.rotation = Quat::from_euler(EulerRot::YXZ, fp.yaw, fp.pitch, 0.0);
}

fn move_input(keys: &ButtonInput<KeyCode>) -> Vec3 {
    let mut v = Vec3::ZERO;
    if keys.pressed(KeyCode::KeyW) { v.z -= 1.0; }
    if keys.pressed(KeyCode::KeyS) { v.z += 1.0; }
    if keys.pressed(KeyCode::KeyA) { v.x -= 1.0; }
    if keys.pressed(KeyCode::KeyD) { v.x += 1.0; }
    v.normalize_or_zero()
}

fn walk(time: Res<Time>, keys: Res<ButtonInput<KeyCode>>, hm: Res<Heightmap>, mut cams: Query<(&FirstPerson, &mut Transform)>) {
    let Ok((fp, mut transform)) = cams.single_mut() else { return };
    let speed = if keys.pressed(KeyCode::ShiftLeft) { RUN_SPEED } else { WALK_SPEED };
    let heading = Quat::from_rotation_y(fp.yaw);
    let step = heading * move_input(&keys) * speed * time.delta_secs();
    let mut p = transform.translation + step;
    p.y = hm.sample(p.x, p.z) + EYE_HEIGHT;
    transform.translation = p;
}

fn fly(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    scroll: Res<AccumulatedMouseScroll>,
    hm: Res<Heightmap>,
    mut cams: Query<(&mut FirstPerson, &mut Transform)>,
) {
    let Ok((mut fp, mut transform)) = cams.single_mut() else { return };
    if scroll.delta.y != 0.0 {
        fp.fly_speed = (fp.fly_speed * 1.15f32.powf(scroll.delta.y.signum())).clamp(2.0, 400.0);
    }
    let mut v = transform.rotation * move_input(&keys);
    if keys.pressed(KeyCode::Space) { v.y += 1.0; }
    if keys.pressed(KeyCode::KeyC) { v.y -= 1.0; }
    let mut p = transform.translation + v * fp.fly_speed * time.delta_secs();
    p.y = p.y.max(hm.sample(p.x, p.z) + 1.0);
    transform.translation = p;
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
