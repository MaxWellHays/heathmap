//! A simple low-poly runner for the player (and later for ghost runners): an articulated
//! figure — pelvis, torso, head, two-part arms and legs, shoes — animated procedurally.
//! Stride frequency and amplitude follow the figure's speed, so it walks when slow and
//! runs when fast (knees lift on the back swing, bent arms swing opposite to the legs,
//! the body leans forward and bobs); in the air the knees tuck.

use bevy::prelude::*;

/// Animation input for a figure: horizontal speed (m/s) and whether it is in the air.
#[derive(Component, Default)]
pub struct Gait {
    pub speed: f32,
    pub airborne: bool,
    phase: f32,
}

#[derive(Component, Clone, Copy, PartialEq)]
enum Joint {
    Pelvis,
    Torso,
    Shoulder(f32), // side: −1 left, +1 right
    Elbow(f32),
    Hip(f32),
    Knee(f32),
}

const HIP_HEIGHT: f32 = 0.92;

pub struct AvatarPlugin;

impl Plugin for AvatarPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, animate);
    }
}

pub struct FigureColors {
    pub shirt: Color,
    pub shorts: Color,
    pub skin: Color,
    pub shoes: Color,
}

impl Default for FigureColors {
    fn default() -> Self {
        Self {
            shirt: Color::srgb(0.95, 0.45, 0.10),
            shorts: Color::srgb(0.12, 0.13, 0.18),
            skin: Color::srgb(0.85, 0.68, 0.55),
            shoes: Color::srgb(0.92, 0.92, 0.92),
        }
    }
}

/// Adds a runner figure as children of `parent` (feet at the parent's origin, facing −z).
pub fn spawn_figure(
    commands: &mut Commands,
    parent: Entity,
    colors: FigureColors,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let mat = |c: Color, m: &mut Assets<StandardMaterial>| m.add(StandardMaterial { base_color: c, perceptual_roughness: 0.8, ..default() });
    let (shirt, shorts, skin, shoes) = (
        mat(colors.shirt, materials),
        mat(colors.shorts, materials),
        mat(colors.skin, materials),
        mat(colors.shoes, materials),
    );
    let torso_mesh = meshes.add(Cuboid::new(0.34, 0.52, 0.2));
    let hips_mesh = meshes.add(Cuboid::new(0.32, 0.16, 0.2));
    let head_mesh = meshes.add(Sphere::new(0.11).mesh().ico(1).unwrap());
    let upper_arm = meshes.add(Capsule3d::new(0.045, 0.22));
    let lower_arm = meshes.add(Capsule3d::new(0.04, 0.2));
    let thigh = meshes.add(Capsule3d::new(0.065, 0.32));
    let shin = meshes.add(Capsule3d::new(0.05, 0.34));
    let foot = meshes.add(Cuboid::new(0.1, 0.07, 0.24));

    let pelvis = commands
        .spawn((Joint::Pelvis, Transform::from_xyz(0.0, HIP_HEIGHT, 0.0), Visibility::Inherited))
        .with_children(|p| {
            p.spawn((Mesh3d(hips_mesh), MeshMaterial3d(shorts.clone()), Transform::from_xyz(0.0, 0.02, 0.0)));
            // Torso pivots at the waist; head and arms hang off it.
            p.spawn((Joint::Torso, Transform::from_xyz(0.0, 0.08, 0.0), Visibility::Inherited)).with_children(|t| {
                t.spawn((Mesh3d(torso_mesh), MeshMaterial3d(shirt.clone()), Transform::from_xyz(0.0, 0.26, 0.0)));
                t.spawn((Mesh3d(head_mesh), MeshMaterial3d(skin.clone()), Transform::from_xyz(0.0, 0.66, 0.0)));
                for side in [-1.0f32, 1.0] {
                    t.spawn((Joint::Shoulder(side), Transform::from_xyz(0.21 * side, 0.48, 0.0), Visibility::Inherited))
                        .with_children(|s| {
                            s.spawn((Mesh3d(upper_arm.clone()), MeshMaterial3d(shirt.clone()), Transform::from_xyz(0.0, -0.13, 0.0)));
                            s.spawn((Joint::Elbow(side), Transform::from_xyz(0.0, -0.28, 0.0), Visibility::Inherited))
                                .with_children(|e| {
                                    e.spawn((Mesh3d(lower_arm.clone()), MeshMaterial3d(skin.clone()), Transform::from_xyz(0.0, -0.13, 0.0)));
                                });
                        });
                }
            });
            for side in [-1.0f32, 1.0] {
                p.spawn((Joint::Hip(side), Transform::from_xyz(0.1 * side, 0.0, 0.0), Visibility::Inherited)).with_children(|h| {
                    h.spawn((Mesh3d(thigh.clone()), MeshMaterial3d(shorts.clone()), Transform::from_xyz(0.0, -0.21, 0.0)));
                    h.spawn((Joint::Knee(side), Transform::from_xyz(0.0, -0.44, 0.0), Visibility::Inherited)).with_children(|k| {
                        k.spawn((Mesh3d(shin.clone()), MeshMaterial3d(skin.clone()), Transform::from_xyz(0.0, -0.22, 0.0)));
                        k.spawn((Mesh3d(foot.clone()), MeshMaterial3d(shoes.clone()), Transform::from_xyz(0.0, -0.45, -0.05)));
                    });
                });
            }
        })
        .id();
    commands.entity(parent).add_child(pelvis);
}

/// Poses every figure's joints from its `Gait`.
fn animate(
    time: Res<Time>,
    mut figures: Query<(Entity, &mut Gait)>,
    children: Query<&Children>,
    mut joints: Query<(&Joint, &mut Transform)>,
) {
    let dt = time.delta_secs();
    for (entity, mut gait) in &mut figures {
        // 0 standing … 1 full run (≈ 4 m/s and faster).
        let run = (gait.speed / 4.0).clamp(0.0, 1.0);
        let moving = (gait.speed / 0.6).clamp(0.0, 1.0);
        // One stride (left + right step) covers more ground the faster you go.
        let stride = 1.3 + 0.35 * gait.speed;
        gait.phase = (gait.phase + gait.speed * dt / stride * std::f32::consts::TAU) % std::f32::consts::TAU;
        let phase = gait.phase;
        let airborne = gait.airborne;

        for joint_entity in children.iter_descendants(entity) {
            let Ok((joint, mut tf)) = joints.get_mut(joint_entity) else { continue };
            // Each side swings half a cycle apart.
            let swing = |side: f32| (phase + if side < 0.0 { 0.0 } else { std::f32::consts::PI }).sin();
            let angle = match *joint {
                Joint::Pelvis => {
                    tf.translation.y = HIP_HEIGHT + 0.05 * run * (2.0 * phase).cos().abs() - 0.03 * run;
                    0.0
                }
                Joint::Torso => -0.18 * run, // lean forward
                Joint::Shoulder(side) => -(0.25 + 0.45 * run) * moving * swing(side),
                Joint::Elbow(_) => 0.25 + 1.1 * run * moving,
                Joint::Hip(side) => {
                    if airborne { 0.6 } else { (0.35 + 0.45 * run) * moving * swing(side) }
                }
                Joint::Knee(side) => {
                    if airborne {
                        -1.1
                    } else {
                        // Knee bends most while the leg swings back and through.
                        let back = (-(phase + if side < 0.0 { 0.0 } else { std::f32::consts::PI } - 0.6).sin()).max(0.0);
                        -(0.1 + (0.3 + 1.2 * run) * back) * moving
                    }
                }
            };
            tf.rotation = Quat::from_rotation_x(angle);
        }
    }
}
