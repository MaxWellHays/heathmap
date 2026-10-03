//! The runner figure for the player and the ghost runners: an artist's wooden mannequin
//! (oval segments joined by ball joints), animated procedurally.
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

/// Colours for an artist's-mannequin figure: wooden segments and slightly darker joints.
pub struct FigureColors {
    pub wood: Color,
    pub joints: Color,
}

impl Default for FigureColors {
    fn default() -> Self {
        Self { wood: Color::srgb(0.85, 0.70, 0.52), joints: Color::srgb(0.70, 0.53, 0.36) }
    }
}

impl FigureColors {
    /// Wood stained towards `tint` (for ghost runners, one colour per run).
    pub fn tinted(tint: Color) -> Self {
        let base = Self::default();
        let mix = |a: Color, t: f32| {
            let (a, b) = (a.to_srgba(), tint.to_srgba());
            Color::srgb(a.red + (b.red - a.red) * t, a.green + (b.green - a.green) * t, a.blue + (b.blue - a.blue) * t)
        };
        Self { wood: mix(base.wood, 0.6), joints: mix(base.joints, 0.45) }
    }
}

/// Adds an artist's-mannequin runner as children of `parent` (feet at the parent's origin,
/// facing −z): oval wooden segments (egg head, neck, chest, waist, pelvis, upper and lower
/// limbs, mitten hands, feet) with ball joints, posed by `animate`.
pub fn spawn_figure(
    commands: &mut Commands,
    parent: Entity,
    colors: FigureColors,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let wood = materials.add(StandardMaterial { base_color: colors.wood, perceptual_roughness: 0.45, reflectance: 0.35, ..default() });
    let joint = materials.add(StandardMaterial { base_color: colors.joints, perceptual_roughness: 0.4, reflectance: 0.4, ..default() });
    // Every segment is the same unit sphere, scaled into an ellipsoid.
    let sphere = meshes.add(Sphere::new(1.0).mesh().uv(20, 14));
    let part = |centre: Vec3, radii: Vec3| (Mesh3d(sphere.clone()), MeshMaterial3d(wood.clone()), Transform::from_translation(centre).with_scale(radii));
    let ball = |centre: Vec3, r: f32| (Mesh3d(sphere.clone()), MeshMaterial3d(joint.clone()), Transform::from_translation(centre).with_scale(Vec3::splat(r)));

    let pelvis = commands
        .spawn((Joint::Pelvis, Transform::from_xyz(0.0, HIP_HEIGHT, 0.0), Visibility::Inherited))
        .with_children(|p| {
            p.spawn(part(Vec3::new(0.0, 0.02, 0.0), Vec3::new(0.16, 0.1, 0.11)));
            // Torso pivots at the waist; head and arms hang off it.
            p.spawn((Joint::Torso, Transform::from_xyz(0.0, 0.08, 0.0), Visibility::Inherited)).with_children(|t| {
                t.spawn(part(Vec3::new(0.0, 0.1, 0.0), Vec3::new(0.12, 0.1, 0.09))); // waist
                t.spawn(part(Vec3::new(0.0, 0.34, 0.0), Vec3::new(0.17, 0.17, 0.11))); // chest
                t.spawn(part(Vec3::new(0.0, 0.56, 0.0), Vec3::new(0.04, 0.06, 0.04))); // neck
                t.spawn(part(Vec3::new(0.0, 0.7, 0.0), Vec3::new(0.085, 0.115, 0.1))); // head
                for side in [-1.0f32, 1.0] {
                    t.spawn((Joint::Shoulder(side), Transform::from_xyz(0.2 * side, 0.46, 0.0), Visibility::Inherited))
                        .with_children(|s| {
                            s.spawn(ball(Vec3::ZERO, 0.05));
                            s.spawn(part(Vec3::new(0.0, -0.14, 0.0), Vec3::new(0.045, 0.13, 0.045)));
                            s.spawn((Joint::Elbow(side), Transform::from_xyz(0.0, -0.28, 0.0), Visibility::Inherited))
                                .with_children(|e| {
                                    e.spawn(ball(Vec3::ZERO, 0.035));
                                    e.spawn(part(Vec3::new(0.0, -0.13, 0.0), Vec3::new(0.038, 0.12, 0.038)));
                                    e.spawn(ball(Vec3::new(0.0, -0.25, 0.0), 0.025)); // wrist
                                    e.spawn(part(Vec3::new(0.0, -0.31, 0.0), Vec3::new(0.035, 0.06, 0.022))); // hand
                                });
                        });
                }
            });
            for side in [-1.0f32, 1.0] {
                p.spawn((Joint::Hip(side), Transform::from_xyz(0.09 * side, 0.0, 0.0), Visibility::Inherited)).with_children(|h| {
                    h.spawn(ball(Vec3::ZERO, 0.055));
                    h.spawn(part(Vec3::new(0.0, -0.22, 0.0), Vec3::new(0.065, 0.2, 0.065)));
                    h.spawn((Joint::Knee(side), Transform::from_xyz(0.0, -0.44, 0.0), Visibility::Inherited)).with_children(|k| {
                        k.spawn(ball(Vec3::ZERO, 0.045));
                        k.spawn(part(Vec3::new(0.0, -0.21, 0.0), Vec3::new(0.05, 0.19, 0.05)));
                        k.spawn(ball(Vec3::new(0.0, -0.42, 0.0), 0.035)); // ankle
                        k.spawn(part(Vec3::new(0.0, -0.46, -0.05), Vec3::new(0.045, 0.03, 0.11))); // foot
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
