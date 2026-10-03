//! Landmarks with their own generators instead of a generic extruded building
//! (listed in data/landmarks.json, exported to landmarks.bin).
//!
//! Pergola (the Hampstead Pergola): a raised Edwardian terrace walk. The LIDAR terrain
//! already contains the terrace itself, so the generator builds what sits on and around
//! it: a paved deck (heights from the highest ground within a few metres, so the 4 m
//! terrain grid can't dip it at the edges), brick retaining walls where the terrace drops
//! to the garden, and along each walkway pairs of stone columns carrying timber beams
//! across and along the walk, with vines on top. Footpaths inside follow the deck, and
//! the deck is walkable.

use bevy::prelude::*;

use crate::buildings::Reader;
use crate::level::{BinaryFile, Heightmap, LevelHandles, LevelState};
use crate::props::{Shapes, srgb};

const COLUMN_SPACING: f32 = 3.5;
const COLUMN_HEIGHT: f32 = 2.9;
const PARAPET: f32 = 0.6; // retaining walls rise this far above the deck
const MIN_DROP: f32 = 0.5; // retaining walls only where the deck is this far above the garden

pub struct Pergola {
    pub outline: Vec<Vec2>,
    pub walks: Vec<Vec<Vec2>>,
}

#[derive(Resource, Default)]
pub struct Landmarks {
    pub pergolas: Vec<Pergola>,
}

impl Landmarks {
    /// The pergola outline containing `p`, if any.
    pub fn pergola_at(&self, p: Vec2) -> Option<&Pergola> {
        self.pergolas.iter().find(|g| point_in_polygon(p, &g.outline))
    }
}

#[derive(Component)]
pub struct LandmarkMeshes;

pub struct LandmarksPlugin;

impl Plugin for LandmarksPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Landmarks>()
            .add_systems(OnEnter(LevelState::Ready), (load_landmarks, spawn_landmarks).chain());
    }
}

pub fn load_landmarks(handles: Res<LevelHandles>, files: Res<Assets<BinaryFile>>, mut landmarks: ResMut<Landmarks>) {
    let Some(file) = handles.landmarks.as_ref().and_then(|h| files.get(h)) else { return };
    let mut r = Reader { bytes: &file.0, pos: 0 };
    for _ in 0..r.u32() {
        let generator = r.u8();
        let outline = (0..r.u32()).map(|_| Vec2::new(r.f32(), r.f32())).collect();
        let walks = (0..r.u16()).map(|_| (0..r.u32()).map(|_| Vec2::new(r.f32(), r.f32())).collect()).collect();
        if generator == 0 {
            landmarks.pergolas.push(Pergola { outline, walks });
        }
    }
}

/// Deck height at `p`: the highest terrain within 3 m (the terrace top), a little above it.
pub fn deck_height(hm: &Heightmap, p: Vec2) -> f32 {
    let mut h = hm.sample(p.x, p.y);
    for r in [1.5, 3.0] {
        for k in 0..8 {
            let a = k as f32 / 8.0 * std::f32::consts::TAU;
            let q = p + Vec2::new(a.cos(), a.sin()) * r;
            h = h.max(hm.sample(q.x, q.y));
        }
    }
    h + 0.05
}

fn spawn_landmarks(
    mut commands: Commands,
    landmarks: Res<Landmarks>,
    hm: Res<Heightmap>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if landmarks.pergolas.is_empty() {
        return;
    }
    let material = materials.add(StandardMaterial {
        perceptual_roughness: 0.85,
        reflectance: 0.15,
        double_sided: true,
        cull_mode: None,
        ..default()
    });
    let mut shapes = Shapes::default();
    for pergola in &landmarks.pergolas {
        build_pergola(&mut shapes, pergola, &hm);
    }
    commands.spawn((LandmarkMeshes, Name::new("Landmarks"), Mesh3d(meshes.add(shapes.build())), MeshMaterial3d(material)));
    info!("Spawned {} pergola(s)", landmarks.pergolas.len());
}

fn build_pergola(s: &mut Shapes, g: &Pergola, hm: &Heightmap) {
    let paving = srgb(0.78, 0.74, 0.66);
    let brick = srgb(0.56, 0.33, 0.25);
    let stone = srgb(0.86, 0.83, 0.75);
    let timber = srgb(0.42, 0.30, 0.19);
    let at = |p: Vec2, y: f32| Vec3::new(p.x, y, p.y);
    let deck = |p: Vec2| deck_height(hm, p);

    // Deck: the outline triangulated, every vertex at the deck height (faces up).
    let flat: Vec<f64> = g.outline.iter().flat_map(|p| [p.x as f64, p.y as f64]).collect();
    if let Ok(tris) = earcutr::earcut(&flat, &[], 2) {
        for t in tris.chunks_exact(3) {
            let (a, b, c) = (g.outline[t[0]], g.outline[t[1]], g.outline[t[2]]);
            let (pa, pb, pc) = (at(a, deck(a)), at(b, deck(b)), at(c, deck(c)));
            if (pb - pa).cross(pc - pa).y >= 0.0 {
                s.triangle(pa, pb, pc, paving);
            } else {
                s.triangle(pa, pc, pb, paving);
            }
        }
    }

    // Retaining walls along the outline wherever the deck stands above the garden.
    let n = g.outline.len();
    for k in 0..n {
        let (a, b) = (g.outline[k], g.outline[(k + 1) % n]);
        let len = a.distance(b);
        let dir = (b - a).normalize_or(Vec2::X);
        let mut out_n = Vec2::new(-dir.y, dir.x);
        if point_in_polygon((a + b) / 2.0 + out_n * 0.3, &g.outline) {
            out_n = -out_n;
        }
        let steps = (len / 1.5).ceil().max(1.0) as usize;
        for i in 0..steps {
            let (p0, p1) = (a.lerp(b, i as f32 / steps as f32), a.lerp(b, (i + 1) as f32 / steps as f32));
            let (d0, d1) = (deck(p0), deck(p1));
            let (o0, o1) = (p0 + out_n * 1.5, p1 + out_n * 1.5);
            let (g0, g1) = (hm.sample(o0.x, o0.y), hm.sample(o1.x, o1.y));
            if d0 - g0 < MIN_DROP && d1 - g1 < MIN_DROP {
                continue;
            }
            let (t0, t1) = (d0 + PARAPET, d1 + PARAPET);
            let (i0, i1) = (p0 - out_n * 0.4, p1 - out_n * 0.4);
            s.quad(at(p0, g0.min(d0) - 0.3), at(p1, g1.min(d1) - 0.3), at(p1, t1), at(p0, t0), brick); // outer face
            s.quad(at(p0, t0), at(p1, t1), at(i1, t1), at(i0, t0), brick); // top
            s.quad(at(i1, d1), at(i0, d0), at(i0, t0), at(i1, t1), brick); // inner face
        }
    }

    // Columns in pairs across each walkway, beams across and along, vines on top.
    for walk in &g.walks {
        let mut sides: [Vec<(Vec2, f32)>; 2] = [Vec::new(), Vec::new()];
        let total: f32 = walk.windows(2).map(|w| w[0].distance(w[1])).sum();
        let mut d = COLUMN_SPACING / 2.0;
        while d < total {
            let (p, dir) = point_and_dir(walk, d);
            let across = Vec2::new(-dir.y, dir.x);
            let mut pair = [None, None];
            for (i, sign) in [1.0f32, -1.0].into_iter().enumerate() {
                if let Some(t) = ray_to_outline(p, across * sign, &g.outline, 6.0) {
                    let t = (t - 0.35).max(0.6);
                    let q = p + across * sign * t;
                    pair[i] = Some((q, deck(q)));
                }
            }
            if let [Some(l), Some(r)] = pair {
                for (side, (q, base)) in [l, r].into_iter().enumerate() {
                    let tf = Transform::from_xyz(q.x, base, q.y);
                    s.cuboid(&tf, Vec3::new(0.0, 0.15, 0.0), Vec3::new(0.26, 0.15, 0.26), stone); // plinth
                    s.cuboid(&tf, Vec3::new(0.0, COLUMN_HEIGHT / 2.0, 0.0), Vec3::new(0.17, COLUMN_HEIGHT / 2.0, 0.17), stone);
                    s.cuboid(&tf, Vec3::new(0.0, COLUMN_HEIGHT, 0.0), Vec3::new(0.24, 0.08, 0.24), stone); // capital
                    sides[side].push((q, base + COLUMN_HEIGHT + 0.08));
                }
                beam(s, l.0, r.0, l.1 + COLUMN_HEIGHT + 0.2, r.1 + COLUMN_HEIGHT + 0.2, timber);
                vines(s, l.0, r.0, l.1.max(r.1) + COLUMN_HEIGHT + 0.35, d);
            }
            d += COLUMN_SPACING;
        }
        // Longitudinal beams along each side, joining consecutive columns.
        for side in &sides {
            for w in side.windows(2) {
                if w[0].0.distance(w[1].0) < COLUMN_SPACING * 2.0 {
                    beam(s, w[0].0, w[1].0, w[0].1 + 0.05, w[1].1 + 0.05, timber);
                }
            }
        }
    }
}

/// A timber beam (0.25 × 0.25 m) from (a, ya) to (b, yb).
fn beam(s: &mut Shapes, a: Vec2, b: Vec2, ya: f32, yb: f32, color: [f32; 4]) {
    let (pa, pb) = (Vec3::new(a.x, ya, a.y), Vec3::new(b.x, yb, b.y));
    let mid = (pa + pb) / 2.0;
    let len = pa.distance(pb);
    if len < 0.1 {
        return;
    }
    let tf = Transform::from_translation(mid).looking_at(pb, Vec3::Y);
    s.cuboid(&tf, Vec3::ZERO, Vec3::new(0.12, 0.12, len / 2.0 + 0.15), color);
}

/// A few leafy clumps along a beam (deterministic pseudo-random sizes and offsets).
fn vines(s: &mut Shapes, a: Vec2, b: Vec2, y: f32, seed: f32) {
    let greens = [srgb(0.24, 0.42, 0.18), srgb(0.30, 0.48, 0.20), srgb(0.20, 0.36, 0.15)];
    for k in 0..4 {
        let h = ((seed * 12.9898 + k as f32 * 78.233).sin() * 43758.547).fract().abs();
        let q = a.lerp(b, 0.15 + 0.7 * (k as f32 + h) / 4.0);
        let size = 0.3 + 0.35 * h;
        let tf = Transform::from_xyz(q.x, y + 0.1 * h, q.y).with_rotation(Quat::from_rotation_y(h * 3.0));
        s.cuboid(&tf, Vec3::ZERO, Vec3::new(size, size * 0.6, size * 0.8), greens[k % 3]);
    }
}

fn point_and_dir(line: &[Vec2], d: f32) -> (Vec2, Vec2) {
    let mut along = 0.0;
    for w in line.windows(2) {
        let len = w[0].distance(w[1]);
        if along + len >= d {
            let t = (d - along) / len.max(1e-6);
            return (w[0].lerp(w[1], t), (w[1] - w[0]).normalize_or(Vec2::X));
        }
        along += len;
    }
    let n = line.len();
    (line[n - 1], (line[n - 1] - line[n - 2]).normalize_or(Vec2::X))
}

/// Distance along the ray (origin, dir) to the outline, within `max`.
fn ray_to_outline(origin: Vec2, dir: Vec2, outline: &[Vec2], max: f32) -> Option<f32> {
    let mut best: Option<f32> = None;
    for k in 0..outline.len() {
        let (a, b) = (outline[k], outline[(k + 1) % outline.len()]);
        let e = b - a;
        let denom = dir.perp_dot(e);
        if denom.abs() < 1e-6 {
            continue;
        }
        let t = (a - origin).perp_dot(e) / denom;
        let u = (a - origin).perp_dot(dir) / denom;
        if t > 0.0 && t <= max && (0.0..=1.0).contains(&u) {
            best = Some(best.map_or(t, |x: f32| x.min(t)));
        }
    }
    best
}

pub fn point_in_polygon(p: Vec2, ring: &[Vec2]) -> bool {
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}
