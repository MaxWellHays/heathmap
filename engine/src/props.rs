//! Props and barriers from OpenStreetMap (exported by data/export_engine.py):
//! benches, drinking fountains, street lamps, traffic signals, bus stops, waste
//! baskets, post boxes, telephone boxes, picnic tables and zebra crossings (with
//! Belisha beacons); fences, walls, hedges and retaining walls following the terrain.
//!
//! Every prop is a handful of coloured boxes, merged into one mesh per chunk (like the
//! trees), drawn within a few hundred metres.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use crate::buildings::Reader;
use crate::level::{BinaryFile, Heightmap, LevelHandles, LevelState};
use crate::lod::LodChunk;

const CHUNK: f32 = 256.0;
const PROP_DRAW_DISTANCE: [f32; 1] = [700.0];
const BARRIER_DRAW_DISTANCE: [f32; 1] = [1500.0];

#[derive(Component)]
pub struct Props;

#[derive(Component)]
pub struct Barriers;

pub struct PropsPlugin;

impl Plugin for PropsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(LevelState::Ready), spawn_props);
    }
}

fn srgb(r: f32, g: f32, b: f32) -> [f32; 4] {
    LinearRgba::from(Color::srgb(r, g, b)).to_f32_array()
}

/// Flat-shaded, vertex-coloured mesh builder.
#[derive(Default)]
struct Shapes {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

impl Shapes {
    fn quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3, color: [f32; 4]) {
        let n = (b - a).cross(d - a).normalize_or_zero();
        let i = self.positions.len() as u32;
        for p in [a, b, c, d] {
            self.positions.push(p.to_array());
            self.normals.push(n.to_array());
            self.colors.push(color);
        }
        self.indices.extend_from_slice(&[i, i + 1, i + 2, i, i + 2, i + 3]);
    }

    /// Axis-aligned box in the prop's own frame, placed by `tf` (position + yaw).
    fn cuboid(&mut self, tf: &Transform, center: Vec3, half: Vec3, color: [f32; 4]) {
        let corner = |sx: f32, sy: f32, sz: f32| tf.transform_point(center + half * Vec3::new(sx, sy, sz));
        let faces = [
            ([1.0, -1.0, -1.0], [1.0, -1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, -1.0]),     // +x
            ([-1.0, -1.0, 1.0], [-1.0, -1.0, -1.0], [-1.0, 1.0, -1.0], [-1.0, 1.0, 1.0]), // −x
            ([-1.0, 1.0, -1.0], [1.0, 1.0, -1.0], [1.0, 1.0, 1.0], [-1.0, 1.0, 1.0]),     // +y
            ([-1.0, -1.0, 1.0], [1.0, -1.0, 1.0], [1.0, -1.0, -1.0], [-1.0, -1.0, -1.0]), // −y
            ([-1.0, -1.0, 1.0], [-1.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, -1.0, 1.0]),     // +z
            ([1.0, -1.0, -1.0], [1.0, 1.0, -1.0], [-1.0, 1.0, -1.0], [-1.0, -1.0, -1.0]), // −z
        ];
        for (a, b, c, d) in faces {
            let p = |v: [f32; 3]| corner(v[0], v[1], v[2]);
            self.quad(p(a), p(b), p(c), p(d), color);
        }
    }

    fn build(self) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors)
            .with_inserted_indices(Indices::U32(self.indices))
    }
}

struct Prop {
    kind: u8,
    variant: u8,
    pos: Vec2,
    yaw: f32,
    size: f32,
}

fn decode_props(bytes: &[u8]) -> Vec<Prop> {
    let mut r = Reader { bytes, pos: 0 };
    (0..r.u32())
        .map(|_| Prop { kind: r.u8(), variant: r.u8(), pos: Vec2::new(r.f32(), r.f32()), yaw: r.f32(), size: r.f32() })
        .collect()
}

struct Barrier {
    kind: u8,
    material: u8,
    height: f32,
    points: Vec<Vec2>,
}

fn decode_barriers(bytes: &[u8]) -> Vec<Barrier> {
    let mut r = Reader { bytes, pos: 0 };
    (0..r.u32())
        .map(|_| {
            let (kind, material, height) = (r.u8(), r.u8(), r.f32());
            let points = (0..r.u32()).map(|_| Vec2::new(r.f32(), r.f32())).collect();
            Barrier { kind, material, height, points }
        })
        .collect()
}

/// Adds one prop in its own frame (front = −z, up = +y, ground at y = 0).
fn add_prop(s: &mut Shapes, p: &Prop, hm: &Heightmap) {
    if p.kind == 8 {
        add_crossing(s, p, hm);
        return;
    }
    let ground = hm.sample(p.pos.x, p.pos.y);
    let tf = Transform::from_xyz(p.pos.x, ground, p.pos.y).with_rotation(Quat::from_rotation_y(p.yaw));
    let wood = srgb(0.52, 0.36, 0.22);
    let metal = srgb(0.18, 0.19, 0.20);
    let stone = srgb(0.62, 0.60, 0.56);
    let red = srgb(0.78, 0.08, 0.08);
    let mut b = |c: Vec3, h: Vec3, col: [f32; 4]| s.cuboid(&tf, c, h, col);
    match p.kind {
        0 => {
            // Bench: seat, back (behind, +z), two leg frames.
            b(Vec3::new(0.0, 0.45, 0.0), Vec3::new(0.8, 0.03, 0.22), wood);
            b(Vec3::new(0.0, 0.75, 0.2), Vec3::new(0.8, 0.15, 0.03), wood);
            for x in [-0.7, 0.7] {
                b(Vec3::new(x, 0.22, 0.0), Vec3::new(0.04, 0.22, 0.2), metal);
            }
        }
        1 => {
            // Drinking fountain: stone pillar with a bowl.
            b(Vec3::new(0.0, 0.45, 0.0), Vec3::new(0.16, 0.45, 0.16), stone);
            b(Vec3::new(0.0, 0.93, 0.0), Vec3::new(0.26, 0.05, 0.26), stone);
            b(Vec3::new(0.0, 1.0, -0.1), Vec3::new(0.03, 0.04, 0.08), metal);
        }
        2 => {
            // Street lamp: pole, arm, head.
            b(Vec3::new(0.0, 3.0, 0.0), Vec3::new(0.07, 3.0, 0.07), metal);
            b(Vec3::new(0.0, 5.95, -0.45), Vec3::new(0.05, 0.05, 0.45), metal);
            b(Vec3::new(0.0, 5.85, -0.85), Vec3::new(0.14, 0.07, 0.25), srgb(0.85, 0.85, 0.80));
        }
        3 => {
            // Traffic signal: pole and head with red, amber and green lamps facing the road.
            b(Vec3::new(0.0, 1.6, 0.0), Vec3::new(0.06, 1.6, 0.06), metal);
            b(Vec3::new(0.0, 3.1, 0.0), Vec3::new(0.15, 0.45, 0.12), srgb(0.05, 0.05, 0.05));
            for (y, c) in [(3.4, srgb(0.9, 0.1, 0.1)), (3.1, srgb(0.95, 0.65, 0.1)), (2.8, srgb(0.1, 0.8, 0.3))] {
                b(Vec3::new(0.0, y, -0.13), Vec3::new(0.08, 0.08, 0.02), c);
            }
        }
        4 => {
            // Bus stop: pole with a red flag; a glass shelter where tagged.
            b(Vec3::new(0.0, 1.4, 0.0), Vec3::new(0.04, 1.4, 0.04), metal);
            b(Vec3::new(0.0, 2.6, 0.0), Vec3::new(0.3, 0.25, 0.02), red);
            if p.variant == 1 {
                b(Vec3::new(1.6, 2.5, 0.6), Vec3::new(1.6, 0.05, 0.7), metal);
                b(Vec3::new(1.6, 1.3, 1.25), Vec3::new(1.6, 1.15, 0.02), srgb(0.70, 0.82, 0.88));
                for x in [0.05, 3.15] {
                    b(Vec3::new(x, 1.25, 0.6), Vec3::new(0.04, 1.25, 0.65), metal);
                }
            }
        }
        5 => b(Vec3::new(0.0, 0.45, 0.0), Vec3::new(0.22, 0.45, 0.22), srgb(0.10, 0.16, 0.12)),
        6 => {
            // Royal Mail pillar box.
            b(Vec3::new(0.0, 0.75, 0.0), Vec3::new(0.25, 0.75, 0.25), red);
            b(Vec3::new(0.0, 1.55, 0.0), Vec3::new(0.28, 0.06, 0.28), red);
        }
        7 => {
            // Picnic table: top, two benches, legs.
            b(Vec3::new(0.0, 0.75, 0.0), Vec3::new(0.9, 0.03, 0.35), wood);
            for z in [-0.6, 0.6] {
                b(Vec3::new(0.0, 0.45, z), Vec3::new(0.9, 0.03, 0.15), wood);
            }
            for x in [-0.75, 0.75] {
                b(Vec3::new(x, 0.37, 0.0), Vec3::new(0.04, 0.37, 0.7), wood);
            }
        }
        9 => {
            // K6 telephone box.
            b(Vec3::new(0.0, 1.25, 0.0), Vec3::new(0.45, 1.25, 0.45), red);
            b(Vec3::new(0.0, 2.55, 0.0), Vec3::new(0.5, 0.06, 0.5), red);
        }
        _ => {}
    }
}

/// Zebra crossing: white bars across the road (each bar runs along the traffic, 3 m long,
/// 0.5 m wide, 0.5 m apart), draped just above the asphalt, plus a Belisha beacon at each end.
fn add_crossing(s: &mut Shapes, p: &Prop, hm: &Heightmap) {
    const LIFT: f32 = 0.26; // above the highest road surface
    let along = Vec2::new(-p.yaw.sin(), -p.yaw.cos()); // road direction
    let across = Vec2::new(-along.y, along.x);
    let length = p.size.max(4.0);
    let white = srgb(0.93, 0.93, 0.90);
    let bars = (length / 1.0).floor() as i32;
    for k in 0..bars {
        let c = p.pos + across * ((k as f32 + 0.5) - bars as f32 / 2.0);
        let corners = [
            c - across * 0.25 - along * 1.5,
            c + across * 0.25 - along * 1.5,
            c + across * 0.25 + along * 1.5,
            c - across * 0.25 + along * 1.5,
        ];
        let at = |q: Vec2| Vec3::new(q.x, hm.sample(q.x, q.y) + LIFT, q.y);
        // Wound to face up for either orientation.
        let (a, b, cc, d) = (at(corners[0]), at(corners[1]), at(corners[2]), at(corners[3]));
        if (b - a).cross(d - a).y >= 0.0 {
            s.quad(a, b, cc, d, white);
        } else {
            s.quad(a, d, cc, b, white);
        }
    }
    for side in [-1.0, 1.0] {
        let q = p.pos + across * side * (length / 2.0 + 0.6);
        let tf = Transform::from_xyz(q.x, hm.sample(q.x, q.y), q.y);
        for k in 0..5 {
            let col = if k % 2 == 0 { srgb(0.05, 0.05, 0.05) } else { srgb(0.95, 0.95, 0.95) };
            s.cuboid(&tf, Vec3::new(0.0, 0.25 + k as f32 * 0.5, 0.0), Vec3::new(0.05, 0.25, 0.05), col);
        }
        s.cuboid(&tf, Vec3::new(0.0, 2.65, 0.0), Vec3::new(0.15, 0.15, 0.15), srgb(1.0, 0.55, 0.05));
    }
}

/// Fence (posts + rails), wall, hedge or retaining wall along a line, following the terrain.
fn add_barrier(s: &mut Shapes, b: &Barrier, hm: &Heightmap) {
    let (default_height, thickness) = match b.kind {
        0 => (if b.material == 1 { 1.8 } else { 1.3 }, 0.05),
        1 => (1.6, 0.3),
        2 => (1.6, 0.9),
        _ => (1.0, 0.4),
    };
    let height = if b.height > 0.2 { b.height } else { default_height };
    let color = match (b.kind, b.material) {
        (0, 1) => srgb(0.12, 0.13, 0.13),
        (0, _) => srgb(0.45, 0.32, 0.20),
        (2, _) => srgb(0.20, 0.38, 0.16),
        (_, 4) => srgb(0.60, 0.58, 0.53),
        (_, 5) => srgb(0.66, 0.66, 0.64),
        (1, _) => srgb(0.62, 0.36, 0.25), // brick
        _ => srgb(0.58, 0.55, 0.50),
    };
    let ground = |q: Vec2| hm.sample(q.x, q.y);
    if b.kind == 0 {
        // Posts every 2.5 m and two rails between them.
        let post = Vec3::new(0.05, height / 2.0, 0.05);
        let mut carry = 0.0;
        for w in b.points.windows(2) {
            let (a, c) = (w[0], w[1]);
            let len = a.distance(c);
            let mut d = carry;
            while d <= len {
                let q = a.lerp(c, d / len.max(1e-6));
                let tf = Transform::from_xyz(q.x, ground(q), q.y);
                s.cuboid(&tf, Vec3::new(0.0, height / 2.0, 0.0), post, color);
                d += 2.5;
            }
            carry = d - len;
            let n = (len / 2.0).ceil().max(1.0) as usize;
            for k in 0..n {
                let (q0, q1) = (a.lerp(c, k as f32 / n as f32), a.lerp(c, (k + 1) as f32 / n as f32));
                for frac in [0.35, 0.9] {
                    rail(s, q0, q1, ground(q0) + height * frac, ground(q1) + height * frac, 0.03, color);
                }
            }
        }
    } else {
        // Solid strip: two sides and a top, every 2 m, base below the ground.
        let half = thickness / 2.0;
        for w in b.points.windows(2) {
            let (a, c) = (w[0], w[1]);
            let len = a.distance(c);
            let dir = (c - a).normalize_or(Vec2::X);
            let side = Vec2::new(-dir.y, dir.x) * half;
            let n = (len / 2.0).ceil().max(1.0) as usize;
            for k in 0..n {
                let (q0, q1) = (a.lerp(c, k as f32 / n as f32), a.lerp(c, (k + 1) as f32 / n as f32));
                let (g0, g1) = (ground(q0), ground(q1));
                let at = |q: Vec2, y: f32| Vec3::new(q.x, y, q.y);
                let (l0, l1, r0, r1) = (q0 + side, q1 + side, q0 - side, q1 - side);
                let (t0, t1) = (g0 + height, g1 + height);
                s.quad(at(l0, g0 - 0.3), at(l1, g1 - 0.3), at(l1, t1), at(l0, t0), color);
                s.quad(at(r1, g1 - 0.3), at(r0, g0 - 0.3), at(r0, t0), at(r1, t1), color);
                s.quad(at(l0, t0), at(l1, t1), at(r1, t1), at(r0, t0), color);
                if k == 0 {
                    s.quad(at(r0, g0 - 0.3), at(l0, g0 - 0.3), at(l0, t0), at(r0, t0), color);
                }
                if k == n - 1 {
                    s.quad(at(l1, g1 - 0.3), at(r1, g1 - 0.3), at(r1, t1), at(l1, t1), color);
                }
            }
        }
    }
}

/// A thin horizontal bar from q0 (height y0) to q1 (height y1).
fn rail(s: &mut Shapes, q0: Vec2, q1: Vec2, y0: f32, y1: f32, half: f32, color: [f32; 4]) {
    let dir = (q1 - q0).normalize_or(Vec2::X);
    let side = Vec2::new(-dir.y, dir.x) * half;
    let at = |q: Vec2, y: f32| Vec3::new(q.x, y, q.y);
    let (l0, l1, r0, r1) = (q0 + side, q1 + side, q0 - side, q1 - side);
    s.quad(at(l0, y0 - half), at(l1, y1 - half), at(l1, y1 + half), at(l0, y0 + half), color);
    s.quad(at(r1, y1 - half), at(r0, y0 - half), at(r0, y0 + half), at(r1, y1 + half), color);
    s.quad(at(l0, y0 + half), at(l1, y1 + half), at(r1, y1 + half), at(r0, y0 + half), color);
    s.quad(at(r0, y0 - half), at(r1, y1 - half), at(l1, y1 - half), at(l0, y0 - half), color);
}

#[allow(clippy::too_many_arguments)]
fn spawn_props(
    mut commands: Commands,
    handles: Res<LevelHandles>,
    files: Res<Assets<BinaryFile>>,
    hm: Res<Heightmap>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let material = materials.add(StandardMaterial {
        perceptual_roughness: 0.8,
        reflectance: 0.2,
        // Hand-built boxes and thin quads: draw both sides so nothing looks hollow.
        double_sided: true,
        cull_mode: None,
        ..default()
    });
    let key = |p: Vec2| ((p.x / CHUNK).floor() as i32, (p.y / CHUNK).floor() as i32);

    let props = handles.props.as_ref().and_then(|h| files.get(h)).map(|f| decode_props(&f.0)).unwrap_or_default();
    let mut prop_chunks: std::collections::HashMap<(i32, i32), Shapes> = default();
    for p in &props {
        add_prop(prop_chunks.entry(key(p.pos)).or_default(), p, &hm);
    }
    let barriers = handles.barriers.as_ref().and_then(|h| files.get(h)).map(|f| decode_barriers(&f.0)).unwrap_or_default();
    let mut barrier_chunks: std::collections::HashMap<(i32, i32), Shapes> = default();
    for b in barriers.iter().filter(|b| b.points.len() >= 2) {
        add_barrier(barrier_chunks.entry(key(b.points[0])).or_default(), b, &hm);
    }

    let props_root = commands.spawn((Props, Name::new("Props"), Transform::default(), Visibility::default())).id();
    let barriers_root = commands.spawn((Barriers, Name::new("Barriers"), Transform::default(), Visibility::default())).id();
    let mut spawn_chunks = |chunks: std::collections::HashMap<(i32, i32), Shapes>, root: Entity, distances: &'static [f32]| {
        let mut triangles = 0;
        for ((cx, cz), shapes) in chunks {
            triangles += shapes.indices.len() / 3;
            let c = Vec2::new((cx as f32 + 0.5) * CHUNK, (cz as f32 + 0.5) * CHUNK);
            let center = Vec3::new(c.x, hm.sample(c.x, c.y), c.y);
            let mesh = commands.spawn((Mesh3d(meshes.add(shapes.build())), MeshMaterial3d(material.clone()))).id();
            let chunk = commands
                .spawn((
                    LodChunk { center, levels: vec![mesh, Entity::PLACEHOLDER], distances },
                    Transform::default(),
                    Visibility::default(),
                ))
                .add_child(mesh)
                .id();
            commands.entity(root).add_child(chunk);
        }
        triangles
    };
    let prop_tris = spawn_chunks(prop_chunks, props_root, &PROP_DRAW_DISTANCE);
    let barrier_tris = spawn_chunks(barrier_chunks, barriers_root, &BARRIER_DRAW_DISTANCE);
    info!(
        "Spawned {} props ({prop_tris} triangles) and {} barriers ({barrier_tris} triangles)",
        props.len(),
        barriers.len()
    );
}
