//! Buildings: extruded OSM footprints with LIDAR heights and generated roofs.
//!
//! Roof shape comes from the LIDAR height distribution inside each footprint: on a
//! pitched roof heights spread between eaves and ridge, so the 90th percentile sits
//! well above the median (for a gable, p90 − p50 ≈ 0.4 × ridge rise); flat roofs
//! have p90 ≈ p50. Pitched roofs are built over the footprint's minimum rotated
//! rectangle — hipped for squarish buildings, gabled for long ones — clipped to the
//! real outline, and walls rise to meet the roof, so L-shapes and terraces work.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use crate::level::{BinaryFile, Heightmap, Level, LevelState};
use crate::lod::LodChunk;

const CHUNK: f32 = 512.0;
const BASE_SINK: f32 = 1.0; // walls start this far below the lowest terrain point
const MAX_PITCHED_AREA: f32 = 2500.0; // m²; larger footprints are commercial/institutional → flat
const MAX_PITCHED_HEIGHT: f32 = 25.0;
const MIN_ROOF_SIGNAL: f32 = 0.7; // m of p90 − p50 needed to call a roof pitched
const HIP_ASPECT: f32 = 1.6; // long/short side ratio below which roofs are hipped
/// Buildings beyond this distance are not drawn at all (the ground texture shows their footprints).
const DRAW_DISTANCE: [f32; 1] = [4500.0];

#[derive(Component)]
pub struct Buildings;

/// One building as loaded from `buildings.bin` (level coordinates).
#[derive(Clone)]
pub struct Building {
    pub ground: f32,
    pub height: f32,
    pub height_p50: f32,
    /// Ring 0 is the outer ring; the rest are courtyards.
    pub rings: Vec<Vec<Vec2>>,
}

/// Footprints and heights kept for picking and (later) collisions.
#[derive(Resource, Default)]
pub struct BuildingIndex {
    pub buildings: Vec<Building>,
}

pub struct BuildingsPlugin;

impl Plugin for BuildingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BuildingIndex>()
            .add_systems(OnEnter(LevelState::Ready), spawn_buildings);
    }
}

pub fn decode_buildings(bytes: &[u8]) -> Vec<Building> {
    let mut r = Reader { bytes, pos: 0 };
    let count = r.u32() as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let (ground, height, height_p50) = (r.f32(), r.f32(), r.f32());
        let rings = (0..r.u16())
            .map(|_| (0..r.u32()).map(|_| Vec2::new(r.f32(), r.f32())).collect())
            .collect();
        out.push(Building { ground, height, height_p50, rings });
    }
    out
}

pub struct Reader<'a> {
    pub bytes: &'a [u8],
    pub pos: usize,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> [u8; N] {
        let v: [u8; N] = self.bytes[self.pos..self.pos + N].try_into().unwrap();
        self.pos += N;
        v
    }
    pub fn u8(&mut self) -> u8 {
        self.take::<1>()[0]
    }
    pub fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.take())
    }
    pub fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take())
    }
    pub fn f32(&mut self) -> f32 {
        f32::from_le_bytes(self.take())
    }
}

fn spawn_buildings(
    mut commands: Commands,
    level: Res<Level>,
    files: Res<Assets<BinaryFile>>,
    handles: Res<crate::level::LevelHandles>,
    mut index: ResMut<BuildingIndex>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(file) = handles.buildings.as_ref().and_then(|h| files.get(h)) else { return };
    let buildings = decode_buildings(&file.0);
    let material = materials.add(StandardMaterial { perceptual_roughness: 0.85, reflectance: 0.2, ..default() });

    let mut chunks: std::collections::HashMap<(i32, i32), MeshBuilder> = default();
    let (mut pitched, mut flat) = (0, 0);
    for b in &buildings {
        let Some(&first) = b.rings[0].first() else { continue };
        let key = ((first.x / CHUNK).floor() as i32, (first.y / CHUNK).floor() as i32);
        let mb = chunks.entry(key).or_default();
        if add_building(mb, b, &level.heightmap) {
            pitched += 1;
        } else {
            flat += 1;
        }
    }
    let root = commands.spawn((Buildings, Name::new("Buildings"), Transform::default(), Visibility::default())).id();
    let mut triangles = 0;
    for ((cx, cz), mb) in chunks {
        triangles += mb.indices.len() / 3;
        let center = Vec2::new((cx as f32 + 0.5) * CHUNK, (cz as f32 + 0.5) * CHUNK);
        let center = Vec3::new(center.x, level.heightmap.sample(center.x, center.y), center.y);
        let mesh = commands.spawn((Mesh3d(meshes.add(mb.build())), MeshMaterial3d(material.clone()))).id();
        let chunk = commands
            .spawn((
                LodChunk { center, levels: vec![mesh, Entity::PLACEHOLDER], distances: &DRAW_DISTANCE },
                Transform::default(),
                Visibility::default(),
            ))
            .add_child(mesh)
            .id();
        commands.entity(root).add_child(chunk);
    }
    info!("Spawned {} buildings ({pitched} pitched, {flat} flat roofs), {triangles} triangles", buildings.len());
    index.buildings = buildings;
}

#[derive(Default)]
struct MeshBuilder {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

impl MeshBuilder {
    /// Adds a flat-shaded polygon (triangle fan or triangulated list), normal from its first triangle.
    fn triangle(&mut self, a: Vec3, b: Vec3, c: Vec3, color: [f32; 4]) {
        let n = (b - a).cross(c - a).normalize_or_zero();
        if n == Vec3::ZERO {
            return;
        }
        let i = self.positions.len() as u32;
        for p in [a, b, c] {
            self.positions.push(p.to_array());
            self.normals.push(n.to_array());
            self.colors.push(color);
        }
        self.indices.extend_from_slice(&[i, i + 1, i + 2]);
    }

    fn quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3, color: [f32; 4]) {
        // a-b bottom edge, d-c top edge (d above a, c above b).
        let n = (b - a).cross(d - a).normalize_or_zero();
        let i = self.positions.len() as u32;
        for p in [a, b, c, d] {
            self.positions.push(p.to_array());
            self.normals.push(n.to_array());
            self.colors.push(color);
        }
        self.indices.extend_from_slice(&[i, i + 1, i + 2, i, i + 2, i + 3]);
    }

    fn build(self) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors)
            .with_inserted_indices(Indices::U32(self.indices))
    }
}

fn srgb(r: f32, g: f32, b: f32) -> [f32; 4] {
    LinearRgba::from(Color::srgb(r, g, b)).to_f32_array()
}

fn hash01(p: Vec2) -> f32 {
    let h = (p.x * 12.9898 + p.y * 78.233).sin() * 43758.547;
    h - h.floor()
}

/// Roof height above the eaves as a function of position: piecewise linear over the
/// minimum rotated rectangle (`u` along the long axis, `v` across it).
struct RoofShape {
    center: Vec2,
    u: Vec2,
    v: Vec2,
    half_long: f32,
    half_short: f32,
    rise: f32,
    hipped: bool,
}

impl RoofShape {
    fn local(&self, p: Vec2) -> (f32, f32) {
        let d = p - self.center;
        (d.dot(self.u), d.dot(self.v))
    }

    fn height(&self, p: Vec2) -> f32 {
        let (du, dv) = self.local(p);
        let slope = self.rise / self.half_short;
        let across = (self.half_short - dv.abs()).max(0.0);
        let along = if self.hipped { (self.half_long - du.abs()).max(0.0) } else { f32::MAX };
        slope * across.min(along)
    }

    /// The roof's planar pieces, each a convex polygon (in level x/z).
    fn regions(&self) -> Vec<Vec<Vec2>> {
        let (a, b) = (self.half_long + 1.0, self.half_short + 1.0); // a little margin around the rect
        let p = |u: f32, v: f32| self.center + self.u * u + self.v * v;
        if !self.hipped {
            return vec![
                vec![p(-a, 0.0), p(a, 0.0), p(a, b), p(-a, b)],
                vec![p(-a, -b), p(a, -b), p(a, 0.0), p(-a, 0.0)],
            ];
        }
        // Hipped: ridge along u from −r to r, hips run diagonally to the corners.
        let r = self.half_long - self.half_short;
        let (ca, cb) = (a + 1.0, b + 1.0);
        vec![
            vec![p(-r, 0.0), p(r, 0.0), p(r + cb, cb), p(-r - cb, cb)],        // +v side
            vec![p(-r - cb, -cb), p(r + cb, -cb), p(r, 0.0), p(-r, 0.0)],      // −v side
            vec![p(r, 0.0), p(r + cb, -cb), p(ca + cb, 0.0), p(r + cb, cb)],   // +u end
            vec![p(-r, 0.0), p(-r - cb, cb), p(-ca - cb, 0.0), p(-r - cb, -cb)], // −u end
        ]
    }

    /// Lines where the roof surface creases (wall tops must break there too).
    fn crease_lines(&self) -> Vec<(Vec2, Vec2)> {
        let d = self.half_long - self.half_short;
        let mut lines = vec![(self.center, self.u)]; // ridge line v = 0
        if self.hipped {
            for (su, sv) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
                let corner = self.center + self.u * (su * d);
                lines.push((corner, (self.u * su + self.v * sv).normalize()));
            }
        }
        lines
    }
}

/// Adds walls and roof for one building; returns true if the roof is pitched.
fn add_building(mb: &mut MeshBuilder, b: &Building, hm: &Heightmap) -> bool {
    let outer = &b.rings[0];
    if outer.len() < 3 {
        return false;
    }
    let base = outer.iter().map(|p| hm.sample(p.x, p.y)).fold(f32::MAX, f32::min) - BASE_SINK;
    let top = b.ground + b.height;
    let area = polygon_area(outer).abs();
    let seed = hash01(outer[0]);
    let wall = srgb(0.80 + 0.08 * seed, 0.72 + 0.06 * seed, 0.58 + 0.04 * seed); // London stock brick, lightened
    let flat_roof = srgb(0.55, 0.55, 0.53);
    let pitched_roof = if seed < 0.6 { srgb(0.42, 0.43, 0.47) } else { srgb(0.58, 0.36, 0.28) }; // slate or tile

    let rect = min_rotated_rect(outer);
    let signal = b.height - b.height_p50;
    let pitched = b.rings.len() == 1
        && area < MAX_PITCHED_AREA
        && b.height < MAX_PITCHED_HEIGHT
        && signal >= MIN_ROOF_SIGNAL
        && rect.is_some();

    if !pitched {
        add_walls(mb, &b.rings, base, |_| top, wall, None);
        add_flat_roof(mb, &b.rings, top, flat_roof);
        return false;
    }

    let (center, u, half_long, half_short) = rect.unwrap();
    let max_rise = (0.6 * b.height).min(1.2 * half_short);
    let rise = (2.5 * signal).clamp(1.0, max_rise.max(1.0));
    let eaves = top - 0.9 * rise;
    let roof = RoofShape {
        center,
        u,
        v: Vec2::new(-u.y, u.x),
        half_long,
        half_short,
        rise,
        hipped: half_long / half_short < HIP_ASPECT,
    };
    add_walls(mb, &b.rings, base, |p| eaves + roof.height(p), wall, Some(&roof));
    for region in roof.regions() {
        let piece = clip_convex(outer, &region);
        if piece.len() < 3 {
            continue;
        }
        for [i, j, k] in triangulate(&[piece.clone()]) {
            let lift = |p: Vec2| Vec3::new(p.x, eaves + roof.height(p), p.y);
            add_upward_triangle(mb, lift(piece[i]), lift(piece[j]), lift(piece[k]), pitched_roof);
        }
    }
    true
}

fn add_upward_triangle(mb: &mut MeshBuilder, a: Vec3, b: Vec3, c: Vec3, color: [f32; 4]) {
    if (b - a).cross(c - a).y >= 0.0 {
        mb.triangle(a, b, c, color);
    } else {
        mb.triangle(a, c, b, color);
    }
}

fn add_flat_roof(mb: &mut MeshBuilder, rings: &[Vec<Vec2>], top: f32, color: [f32; 4]) {
    let flat: Vec<Vec2> = rings.iter().flatten().copied().collect();
    for [i, j, k] in triangulate(rings) {
        let lift = |p: Vec2| Vec3::new(p.x, top, p.y);
        add_upward_triangle(mb, lift(flat[i]), lift(flat[j]), lift(flat[k]), color);
    }
}

/// Walls along every ring edge, from `base` up to `top_at(p)`. With a roof, edges are
/// split where they cross roof creases so wall tops follow the roof exactly.
fn add_walls(
    mb: &mut MeshBuilder,
    rings: &[Vec<Vec2>],
    base: f32,
    top_at: impl Fn(Vec2) -> f32,
    color: [f32; 4],
    roof: Option<&RoofShape>,
) {
    for (ri, ring) in rings.iter().enumerate() {
        // Outer ring walls face outward, courtyard walls face into the courtyard.
        let ccw = polygon_area(ring) > 0.0;
        let outward_flip = ccw == (ri == 0);
        for k in 0..ring.len() {
            let (p, q) = (ring[k], ring[(k + 1) % ring.len()]);
            let mut ts = vec![0.0, 1.0];
            if let Some(roof) = roof {
                for (origin, dir) in roof.crease_lines() {
                    if let Some(t) = segment_line_intersection(p, q, origin, dir) {
                        ts.push(t);
                    }
                }
                ts.sort_by(f32::total_cmp);
            }
            for w in ts.windows(2) {
                let (a2, b2) = (p.lerp(q, w[0]), p.lerp(q, w[1]));
                if a2.distance_squared(b2) < 1e-4 {
                    continue;
                }
                let (a2, b2) = if outward_flip { (b2, a2) } else { (a2, b2) };
                let (a, b) = (Vec3::new(a2.x, base, a2.y), Vec3::new(b2.x, base, b2.y));
                let (c, d) = (Vec3::new(b2.x, top_at(b2), b2.y), Vec3::new(a2.x, top_at(a2), a2.y));
                mb.quad(a, b, c, d, color);
            }
        }
    }
}

/// Signed area in the x/z plane (positive = counter-clockwise when viewed from above, +y).
/// Our z axis points south, so "viewed from above" flips the usual x/y sign.
fn polygon_area(ring: &[Vec2]) -> f32 {
    let mut a = 0.0;
    for k in 0..ring.len() {
        let (p, q) = (ring[k], ring[(k + 1) % ring.len()]);
        a += p.x * q.y - q.x * p.y;
    }
    -a / 2.0
}

fn segment_line_intersection(p: Vec2, q: Vec2, origin: Vec2, dir: Vec2) -> Option<f32> {
    let e = q - p;
    let denom = e.perp_dot(dir);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (origin - p).perp_dot(dir) / denom;
    (t > 1e-4 && t < 1.0 - 1e-4).then_some(t)
}

/// Minimum-area rotated rectangle: (center, long-axis unit vector, half long, half short).
fn min_rotated_rect(points: &[Vec2]) -> Option<(Vec2, Vec2, f32, f32)> {
    let hull = convex_hull(points);
    if hull.len() < 3 {
        return None;
    }
    let mut best: Option<(f32, Vec2, Vec2, f32, f32)> = None;
    for k in 0..hull.len() {
        let e = (hull[(k + 1) % hull.len()] - hull[k]).normalize_or_zero();
        if e == Vec2::ZERO {
            continue;
        }
        let n = Vec2::new(-e.y, e.x);
        let (mut u0, mut u1, mut v0, mut v1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in &hull {
            let (u, v) = (p.dot(e), p.dot(n));
            u0 = u0.min(u);
            u1 = u1.max(u);
            v0 = v0.min(v);
            v1 = v1.max(v);
        }
        let area = (u1 - u0) * (v1 - v0);
        if best.is_none_or(|b| area < b.0) {
            let center = e * (u0 + u1) / 2.0 + n * (v0 + v1) / 2.0;
            let (hu, hv) = ((u1 - u0) / 2.0, (v1 - v0) / 2.0);
            best = Some(if hu >= hv { (area, center, e, hu, hv) } else { (area, center, n, hv, hu) });
        }
    }
    best.filter(|b| b.4 > 0.5).map(|b| (b.1, b.2, b.3, b.4))
}

fn convex_hull(points: &[Vec2]) -> Vec<Vec2> {
    let mut pts = points.to_vec();
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: Vec2, a: Vec2, b: Vec2| (a - o).perp_dot(b - o);
    let mut hull: Vec<Vec2> = Vec::new();
    for pass in 0..2 {
        let start = hull.len();
        let iter: Box<dyn Iterator<Item = &Vec2>> = if pass == 0 { Box::new(pts.iter()) } else { Box::new(pts.iter().rev()) };
        for &p in iter {
            while hull.len() >= start + 2 && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0 {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop();
    }
    hull
}

/// Sutherland–Hodgman: clips a (possibly concave) polygon against a convex one.
fn clip_convex(subject: &[Vec2], clip: &[Vec2]) -> Vec<Vec2> {
    let clip_ccw = {
        let mut a = 0.0;
        for k in 0..clip.len() {
            a += clip[k].perp_dot(clip[(k + 1) % clip.len()]);
        }
        a > 0.0
    };
    let mut output = subject.to_vec();
    for k in 0..clip.len() {
        let (a, b) = (clip[k], clip[(k + 1) % clip.len()]);
        let inside = |p: Vec2| {
            let s = (b - a).perp_dot(p - a);
            if clip_ccw { s >= 0.0 } else { s <= 0.0 }
        };
        let input = std::mem::take(&mut output);
        for i in 0..input.len() {
            let (p, q) = (input[i], input[(i + 1) % input.len()]);
            let (pin, qin) = (inside(p), inside(q));
            if pin {
                output.push(p);
            }
            if pin != qin {
                let e = q - p;
                let t = (b - a).perp_dot(a - p) / (b - a).perp_dot(e);
                output.push(p + e * t);
            }
        }
        if output.is_empty() {
            break;
        }
    }
    output
}

/// Ear-clipping triangulation of a polygon with holes; indices into the rings flattened in order.
fn triangulate(rings: &[Vec<Vec2>]) -> Vec<[usize; 3]> {
    let mut flat = Vec::new();
    let mut holes = Vec::new();
    for (i, ring) in rings.iter().enumerate() {
        if i > 0 {
            holes.push(flat.len() / 2);
        }
        for p in ring {
            flat.extend_from_slice(&[p.x as f64, p.y as f64]);
        }
    }
    let Ok(idx) = earcutr::earcut(&flat, &holes, 2) else { return Vec::new() };
    idx.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect()
}
