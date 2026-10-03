//! Roads and paths as ribbon meshes draped on the terrain, so they stay sharp at any
//! distance (unlike the ground texture). Each ribbon follows the terrain mesh exactly
//! (heights sampled every few metres) and sits slightly above it; materials use
//! procedurally generated, mipmapped, anisotropically filtered textures — asphalt
//! with dashed centre lines for two-way roads, paving, gravel and dirt for paths.

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::image::ImageAddressMode;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;

use crate::buildings::Reader;
use crate::level::{BinaryFile, Heightmap, LevelHandles, LevelState};
use crate::lod::LodChunk;
use crate::textures::image_with_mips;

const CHUNK: f32 = 512.0;
const MAX_SEGMENT: f32 = 3.0; // m; ribbons are resampled this finely to follow the terrain
const DRAW_DISTANCE: [f32; 1] = [3000.0];
const CENTRE_LINE_WIDTH: f32 = 0.15;
/// Centre lines stop this far from junctions, as painted lines do.
const JUNCTION_CLEARANCE: f32 = 8.0;

/// Must match LINE_KINDS in data/export_engine.py.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Kind {
    RoadMajor,
    RoadMinor,
    Service,
    Pedestrian,
    PathPaved,
    PathUnpaved,
    Steps,
    Track,
}

impl Kind {
    fn from_u8(v: u8) -> Option<Self> {
        use Kind::*;
        [RoadMajor, RoadMinor, Service, Pedestrian, PathPaved, PathUnpaved, Steps, Track].get(v as usize).copied()
    }

    /// Height above the terrain; wider/more important lines sit on top at junctions.
    fn lift(self) -> f32 {
        match self {
            Kind::RoadMajor => 0.16,
            Kind::RoadMinor => 0.14,
            Kind::Service | Kind::Pedestrian => 0.12,
            _ => 0.10,
        }
    }

    /// Length of road covered by one texture repeat (m).
    fn repeat(self) -> f32 {
        match self {
            Kind::RoadMajor | Kind::RoadMinor => 12.0, // 6 m dash, 6 m gap
            _ => 4.0,
        }
    }
}

struct Line {
    kind: Kind,
    oneway: bool,
    lanes: u8,
    width: f32,
    points: Vec<Vec2>,
}

fn decode_lines(bytes: &[u8]) -> Vec<Line> {
    let mut r = Reader { bytes, pos: 0 };
    let count = r.u32() as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let (kind, flags, lanes, width) = (r.u8(), r.u8(), r.u8(), r.f32());
        let points = (0..r.u32()).map(|_| Vec2::new(r.f32(), r.f32())).collect();
        if let Some(kind) = Kind::from_u8(kind) {
            out.push(Line { kind, oneway: flags & 1 != 0, lanes, width, points });
        }
    }
    out
}

#[derive(Component)]
pub struct Lines;

pub struct LinesPlugin;

impl Plugin for LinesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(LevelState::Ready), spawn_lines);
    }
}

/// Which texture a line uses.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Look {
    Asphalt,
    /// Dashed white centre line: a narrow strip, transparent between dashes.
    CentreLine,
    Paving,
    Gravel,
    Dirt,
    Stone,
}

fn look(line: &Line) -> Look {
    match line.kind {
        Kind::RoadMajor | Kind::RoadMinor | Kind::Service => Look::Asphalt,
        Kind::Pedestrian | Kind::PathPaved => Look::Paving,
        Kind::Track => Look::Gravel,
        Kind::PathUnpaved => Look::Dirt,
        Kind::Steps => Look::Stone,
    }
}

fn spawn_lines(
    mut commands: Commands,
    handles: Res<LevelHandles>,
    files: Res<Assets<BinaryFile>>,
    heightmap: Res<Heightmap>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(file) = handles.lines.as_ref().and_then(|h| files.get(h)) else { return };
    let lines = decode_lines(&file.0);

    let mut material_for = std::collections::HashMap::new();
    for l in [Look::Asphalt, Look::CentreLine, Look::Paving, Look::Gravel, Look::Dirt, Look::Stone] {
        let image = images.add(texture(l));
        let m = materials.add(StandardMaterial {
            base_color_texture: Some(image),
            perceptual_roughness: 0.95,
            reflectance: 0.1,
            depth_bias: if l == Look::CentreLine { 60.0 } else { 50.0 },
            alpha_mode: if l == Look::CentreLine { AlphaMode::Mask(0.5) } else { AlphaMode::Opaque },
            ..default()
        });
        material_for.insert(l, m);
    }

    let junctions = road_junctions(&lines);

    // One mesh per (chunk, look).
    let mut builders: std::collections::HashMap<((i32, i32), Look), Ribbons> = default();
    for line in &lines {
        let Some(&first) = line.points.first() else { continue };
        let key = ((first.x / CHUNK).floor() as i32, (first.y / CHUNK).floor() as i32);
        // Square caps (extending the ends by half the width) fill the wedges at road
        // junctions, where overlapping asphalt looks seamless. Paths meet paths of other
        // surfaces, so their caps would show as protruding rectangles.
        let lift = line.kind.lift();
        let caps = look(line) == Look::Asphalt;
        builders.entry((key, look(line))).or_default().add(&line.points, line.width, lift, line.kind.repeat(), caps, &heightmap);
        if has_centre_line(line) {
            let ribbons = builders.entry((key, Look::CentreLine)).or_default();
            for run in marking_runs(&line.points, &junctions) {
                ribbons.add(&run, CENTRE_LINE_WIDTH, lift + 0.02, line.kind.repeat(), false, &heightmap);
            }
        }
    }
    // Round asphalt patches at junctions fill the slivers left between roads meeting at sharp angles.
    for (centre, width) in junctions.values() {
        let key = ((centre.x / CHUNK).floor() as i32, (centre.y / CHUNK).floor() as i32);
        builders.entry((key, Look::Asphalt)).or_default().disc(*centre, width / 2.0, Kind::RoadMajor.lift(), &heightmap);
    }
    let root = commands.spawn((Lines, Name::new("Roads and paths"), Transform::default(), Visibility::default())).id();
    let mut triangles = 0;
    let mut per_chunk: std::collections::HashMap<(i32, i32), Vec<Entity>> = default();
    for ((key, l), ribbons) in builders {
        triangles += ribbons.indices.len() / 3;
        // Flat decals on the ground: casting shadows would only draw a dark line along their edges.
        let e = commands
            .spawn((Mesh3d(meshes.add(ribbons.build())), MeshMaterial3d(material_for[&l].clone()), NotShadowCaster))
            .id();
        per_chunk.entry(key).or_default().push(e);
    }
    for ((cx, cz), entities) in per_chunk {
        let c = Vec2::new((cx as f32 + 0.5) * CHUNK, (cz as f32 + 0.5) * CHUNK);
        let center = Vec3::new(c.x, heightmap.sample(c.x, c.y), c.y);
        // A group entity holds this chunk's meshes so the LOD system can hide them together.
        let group = commands.spawn((Transform::default(), Visibility::default())).add_children(&entities).id();
        let chunk = commands
            .spawn((
                LodChunk { center, levels: vec![group, Entity::PLACEHOLDER], distances: &DRAW_DISTANCE },
                Transform::default(),
                Visibility::default(),
            ))
            .add_child(group)
            .id();
        commands.entity(root).add_child(chunk);
    }
    info!("Spawned {} roads and paths, {triangles} triangles", lines.len());
}

fn has_centre_line(line: &Line) -> bool {
    matches!(line.kind, Kind::RoadMajor | Kind::RoadMinor) && !line.oneway && line.lanes != 1
}

fn vertex_key(p: Vec2) -> (i32, i32) {
    ((p.x * 10.0).round() as i32, (p.y * 10.0).round() as i32)
}

/// Road vertices where three or more road segments meet (OSM ways share junction nodes),
/// with their position and the widest road meeting there. Two segments meeting is just
/// a way continuing, which needs no gap in the markings.
fn road_junctions(lines: &[Line]) -> std::collections::HashMap<(i32, i32), (Vec2, f32)> {
    let mut nodes: std::collections::HashMap<(i32, i32), (Vec2, f32, u32)> = default();
    for line in lines.iter().filter(|l| matches!(l.kind, Kind::RoadMajor | Kind::RoadMinor | Kind::Service)) {
        let n = line.points.len();
        for (i, p) in line.points.iter().enumerate() {
            let e = nodes.entry(vertex_key(*p)).or_insert((*p, 0.0, 0));
            e.1 = e.1.max(line.width);
            e.2 += if i == 0 || i == n - 1 { 1 } else { 2 };
        }
    }
    nodes.into_iter().filter(|(_, (_, _, d))| *d >= 3).map(|(k, (p, w, _))| (k, (p, w))).collect()
}

/// Pieces of a polyline at least `JUNCTION_CLEARANCE` away (along the line) from junctions.
fn marking_runs(points: &[Vec2], junctions: &std::collections::HashMap<(i32, i32), (Vec2, f32)>) -> Vec<Vec<Vec2>> {
    let mut along = vec![0.0f32];
    for w in points.windows(2) {
        along.push(along.last().unwrap() + w[0].distance(w[1]));
    }
    let blocked: Vec<f32> = points
        .iter()
        .zip(&along)
        .filter(|(p, _)| junctions.contains_key(&vertex_key(**p)))
        .map(|(_, &d)| d)
        .collect();
    let total = *along.last().unwrap();
    // Free intervals along the line.
    let mut intervals = vec![(0.0, total)];
    for b in blocked {
        let (lo, hi) = (b - JUNCTION_CLEARANCE, b + JUNCTION_CLEARANCE);
        intervals = intervals
            .into_iter()
            .flat_map(|(s, e): (f32, f32)| {
                let mut out = Vec::new();
                if lo > s { out.push((s, lo.min(e))); }
                if hi < e { out.push((hi.max(s), e)); }
                out
            })
            .filter(|(s, e)| e - s > 1.0)
            .collect();
    }
    let point_at = |d: f32| {
        let k = along.partition_point(|&a| a <= d).clamp(1, points.len() - 1);
        let t = (d - along[k - 1]) / (along[k] - along[k - 1]).max(1e-6);
        points[k - 1].lerp(points[k], t.clamp(0.0, 1.0))
    };
    intervals
        .into_iter()
        .map(|(s, e)| {
            let mut run = vec![point_at(s)];
            run.extend(points.iter().zip(&along).filter(|(_, d)| **d > s && **d < e).map(|(p, _)| *p));
            run.push(point_at(e));
            run
        })
        .collect()
}

#[derive(Default)]
struct Ribbons {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl Ribbons {
    fn add(&mut self, points: &[Vec2], width: f32, lift: f32, repeat: f32, caps: bool, hm: &Heightmap) {
        if points.len() < 2 {
            return;
        }
        let half = width / 2.0;
        let mut points = points.to_vec();
        if caps {
            let n = points.len();
            let start_dir = (points[0] - points[1]).normalize_or_zero();
            let end_dir = (points[n - 1] - points[n - 2]).normalize_or_zero();
            points[0] += start_dir * half;
            points[n - 1] += end_dir * half;
        }
        // Resample so the ribbon follows the terrain between OSM vertices.
        let mut pts = Vec::with_capacity(points.len() * 2);
        for w in points.windows(2) {
            let n = ((w[1] - w[0]).length() / MAX_SEGMENT).ceil().max(1.0) as usize;
            for k in 0..n {
                pts.push(w[0].lerp(w[1], k as f32 / n as f32));
            }
        }
        pts.push(*points.last().unwrap());
        pts.dedup_by(|a, b| a.distance_squared(*b) < 1e-4);
        if pts.len() < 2 {
            return;
        }
        let start = self.positions.len() as u32;
        let mut along = 0.0;
        for i in 0..pts.len() {
            if i > 0 {
                along += pts[i].distance(pts[i - 1]);
            }
            // Offset direction: averaged segment normals with a mitre limit at sharp turns.
            let dir_in = if i > 0 { (pts[i] - pts[i - 1]).normalize_or_zero() } else { Vec2::ZERO };
            let dir_out = if i + 1 < pts.len() { (pts[i + 1] - pts[i]).normalize_or_zero() } else { Vec2::ZERO };
            let tangent = (dir_in + dir_out).normalize_or(dir_in + dir_out);
            let normal = Vec2::new(-tangent.y, tangent.x);
            let seg_normal = Vec2::new(-dir_out.y, dir_out.x).normalize_or(Vec2::new(-dir_in.y, dir_in.x));
            let mitre = 1.0 / normal.dot(seg_normal).abs().max(0.5);
            for (side, u) in [(-1.0, 0.0), (1.0, 1.0)] {
                let p = pts[i] + normal * (half * mitre * side);
                self.positions.push([p.x, hm.sample(p.x, p.y) + lift, p.y]);
                self.normals.push([0.0, 1.0, 0.0]);
                self.uvs.push([u, along / repeat]);
            }
        }
        for i in 0..pts.len() as u32 - 1 {
            let (l0, r0, l1, r1) = (start + 2 * i, start + 2 * i + 1, start + 2 * i + 2, start + 2 * i + 3);
            // Wind both triangles to face up regardless of the line's direction.
            let up = |a: u32, b: u32, c: u32| {
                let p = |k: u32| Vec3::from_array(self.positions[k as usize]);
                (p(b) - p(a)).cross(p(c) - p(a)).y >= 0.0
            };
            for (a, b, c) in [(l0, r0, l1), (r0, r1, l1)] {
                if up(a, b, c) { self.indices.extend_from_slice(&[a, b, c]) } else { self.indices.extend_from_slice(&[a, c, b]) }
            }
        }
    }

    /// A flat disc draped on the terrain (a fan of triangles facing up).
    fn disc(&mut self, centre: Vec2, radius: f32, lift: f32, hm: &Heightmap) {
        const SIDES: usize = 16;
        let start = self.positions.len() as u32;
        self.positions.push([centre.x, hm.sample(centre.x, centre.y) + lift, centre.y]);
        self.normals.push([0.0, 1.0, 0.0]);
        self.uvs.push([0.5, 0.0]);
        for k in 0..SIDES {
            let a = k as f32 / SIDES as f32 * std::f32::consts::TAU;
            let p = centre + Vec2::new(a.cos(), a.sin()) * radius;
            self.positions.push([p.x, hm.sample(p.x, p.y) + lift, p.y]);
            self.normals.push([0.0, 1.0, 0.0]);
            self.uvs.push([0.5, 0.0]);
        }
        for k in 0..SIDES as u32 {
            // Angles increase from +x towards +z (south), i.e. clockwise seen from above:
            // centre, next, current is counter-clockwise from above, so it faces up.
            let (cur, next) = (start + 1 + k, start + 1 + (k + 1) % SIDES as u32);
            self.indices.extend_from_slice(&[start, next, cur]);
        }
    }

    fn build(self) -> Mesh {
        Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
            .with_inserted_indices(Indices::U32(self.indices))
    }
}

/// Procedural texture: `u` runs across the line (0..1), `v` along it (one repeat).
fn texture(look: Look) -> Image {
    const W: usize = 64;
    const H: usize = 256;
    let noise = |x: usize, y: usize| {
        let h = ((x as f32 * 12.9898 + y as f32 * 78.233).sin() * 43758.547).fract().abs();
        h - 0.5
    };
    let mut rgba = vec![0u8; W * H * 4];
    for y in 0..H {
        for x in 0..W {
            let u = (x as f32 + 0.5) / W as f32;
            let v = (y as f32 + 0.5) / H as f32;
            let n = noise(x, y);
            let (r, g, b) = match look {
                Look::Asphalt => (0.30 + 0.05 * n, 0.30 + 0.05 * n, 0.32 + 0.05 * n),
                Look::CentreLine => (0.93, 0.93, 0.90),
                Look::Paving => {
                    // Slabs: darker joints every 1/4 across and 1/8 along.
                    let joint = (u * 4.0).fract() < 0.04 || (v * 8.0).fract() < 0.03;
                    let s = if joint { 0.62 } else { 0.76 + 0.04 * n };
                    (s, s * 0.97, s * 0.92)
                }
                Look::Gravel => (0.62 + 0.12 * n, 0.56 + 0.12 * n, 0.46 + 0.10 * n),
                Look::Dirt => (0.52 + 0.08 * n, 0.42 + 0.07 * n, 0.30 + 0.05 * n),
                Look::Stone => {
                    let step = (v * 8.0).fract() < 0.12;
                    let s = if step { 0.50 } else { 0.66 + 0.04 * n };
                    (s, s, s * 0.97)
                }
            };
            // Centre line: 6 m dash, 6 m gap (one repeat is 12 m).
            let alpha = if look == Look::CentreLine && v >= 0.5 { 0 } else { 255 };
            let i = (y * W + x) * 4;
            let to8 = |c: f32| (c.clamp(0.0, 1.0) * 255.0) as u8;
            rgba[i..i + 4].copy_from_slice(&[to8(r), to8(g), to8(b), alpha]);
        }
    }
    image_with_mips(W, H, rgba, TextureFormat::Rgba8UnormSrgb, ImageAddressMode::ClampToEdge, ImageAddressMode::Repeat)
}
