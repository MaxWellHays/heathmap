//! Roads and paths as ribbon meshes draped on the terrain, so they stay sharp at any
//! distance (unlike the ground texture). Each ribbon follows the terrain mesh exactly
//! (heights sampled every few metres) and sits slightly above it; materials use
//! procedurally generated, mipmapped, anisotropically filtered textures — asphalt
//! with dashed centre lines for two-way roads, paving, gravel and dirt for paths.

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::image::ImageAddressMode;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;
use bevy::render::render_resource::TextureFormat;

use crate::buildings::Reader;
use crate::ground::{ElevationTargets, ElevationUniform, HeightTexture, create_height_texture};
use crate::level::{BinaryFile, Heightmap, LevelHandles, LevelState};
use crate::landmarks::{Landmarks, deck_height, load_landmarks};
use crate::lod::LodChunk;
use crate::textures::image_with_mips;

const CHUNK: f32 = 512.0;
const MAX_SEGMENT: f32 = 2.0; // m; ribbons are resampled this finely along their length…
const ACROSS_SPACING: f32 = 1.5; // m; …and across their width, to follow the terrain
const SKIRT_DEPTH: f32 = 0.3; // m below the terrain that strip edges reach
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

    /// Height above the terrain. Every surface gets its own height, so where strips of
    /// different surfaces overlap (junctions, end caps) one always wins cleanly instead of
    /// flickering; more important lines sit on top. The 1 cm steps are well within depth
    /// buffer precision at viewing distances (reverse-Z float depth). Strips follow the
    /// terrain closely (vertices every 1.5–2 m) and have skirts down into the ground, so
    /// the lift can stay small without the terrain showing through or a gap showing below.
    fn lift(self) -> f32 {
        match self {
            Kind::RoadMajor => 0.12,
            Kind::RoadMinor => 0.11,
            Kind::Service => 0.10,
            Kind::Pedestrian => 0.09,
            Kind::PathPaved => 0.08,
            Kind::Track => 0.07,
            Kind::PathUnpaved => 0.06,
            Kind::Steps => 0.05,
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
    /// OSM bridge=*: the LIDAR terrain has bridges removed, so the deck is built in the air.
    bridge: bool,
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
            out.push(Line { kind, oneway: flags & 1 != 0, bridge: flags & 2 != 0, lanes, width, points });
        }
    }
    out
}

#[derive(Component)]
pub struct Lines;

/// A bridge deck you can walk on: centre line, width, and the straight height profile.
pub struct Deck {
    points: Vec<Vec2>,
    half_width: f32,
    h0: f32,
    h1: f32,
    total: f32,
}

/// All bridge decks, for walking over bridges (the terrain under them is the valley floor).
#[derive(Resource, Default)]
pub struct BridgeDecks(pub Vec<Deck>);

impl BridgeDecks {
    /// Height of the deck surface at `p`, if `p` is on a deck.
    pub fn height_at(&self, p: Vec2) -> Option<f32> {
        let mut best: Option<f32> = None;
        for d in &self.0 {
            let mut along = 0.0;
            for w in d.points.windows(2) {
                let (a, b) = (w[0], w[1]);
                let len = a.distance(b);
                let t = ((p - a).dot(b - a) / (len * len).max(1e-6)).clamp(0.0, 1.0);
                if p.distance(a.lerp(b, t)) <= d.half_width {
                    let h = d.h0 + (d.h1 - d.h0) * ((along + t * len) / d.total.max(1e-3));
                    best = Some(best.map_or(h, |x: f32| x.max(h)));
                }
                along += len;
            }
        }
        best
    }
}

/// Road and path material: standard textured surface plus optional elevation tint.
pub type LinesMaterial = ExtendedMaterial<StandardMaterial, LineTint>;

#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct LineTint {
    #[uniform(100)]
    pub elevation: ElevationUniform,
    /// Heightmap placement (as in the terrain shader), so contours on roads match the terrain's.
    #[uniform(101)]
    pub height_grid: Vec4,
    #[uniform(102)]
    pub height_size: Vec4,
    #[texture(103, sample_type = "float", filterable = false)]
    pub height: Handle<Image>,
}

impl MaterialExtension for LineTint {
    fn fragment_shader() -> ShaderRef {
        "shaders/lines.wgsl".into()
    }
}

pub struct LinesPlugin;

impl Plugin for LinesPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<LinesMaterial>::default())
            .init_resource::<BridgeDecks>()
            .add_systems(OnEnter(LevelState::Ready), spawn_lines.after(create_height_texture).after(load_landmarks));
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
    /// 3D stair treads and risers.
    Stone,
    /// 3D bridge structure: slab, parapets, piers.
    Masonry,
}

impl Look {
    /// Flat strips on the ground don't cast shadows (only a dark line along their edges);
    /// stairs and bridges are real 3D shapes and do.
    fn casts_shadows(self) -> bool {
        matches!(self, Look::Stone | Look::Masonry)
    }
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
    mut materials: ResMut<Assets<LinesMaterial>>,
    mut targets: ResMut<ElevationTargets>,
    height: Res<HeightTexture>,
    mut decks: ResMut<BridgeDecks>,
    landmarks: Res<Landmarks>,
) {
    let Some(file) = handles.lines.as_ref().and_then(|h| files.get(h)) else { return };
    let lines = decode_lines(&file.0);

    let mut material_for = std::collections::HashMap::new();
    for l in [Look::Asphalt, Look::CentreLine, Look::Paving, Look::Gravel, Look::Dirt, Look::Stone, Look::Masonry] {
        let image = images.add(texture(l));
        let base = StandardMaterial {
            base_color_texture: Some(image),
            perceptual_roughness: 0.95,
            reflectance: 0.1,
            depth_bias: if l == Look::CentreLine { 60.0 } else { 50.0 },
            alpha_mode: if l == Look::CentreLine { AlphaMode::Mask(0.5) } else { AlphaMode::Opaque },
            // Stairs and bridges are built face by face; drawing both sides keeps them solid.
            double_sided: l.casts_shadows(),
            cull_mode: if l.casts_shadows() { None } else { Some(bevy::render::render_resource::Face::Back) },
            ..default()
        };
        let extension = LineTint {
            elevation: ElevationUniform::default(),
            height_grid: height.grid,
            height_size: height.size,
            height: height.image.clone(),
        };
        let m = materials.add(LinesMaterial { base, extension });
        targets.lines.push(m.clone());
        material_for.insert(l, m);
    }

    let junctions = road_junctions(&lines);
    // Widest non-step line passing *through* each vertex: stairs ending on a path or road
    // that continues past them stop at its edge instead of running over it. Where a path
    // simply ends at the stairs, they join end to end and nothing is trimmed.
    let mut width_at: std::collections::HashMap<(i32, i32), f32> = default();
    for l in lines.iter().filter(|l| l.kind != Kind::Steps && l.points.len() > 2) {
        for p in &l.points[1..l.points.len() - 1] {
            let w = width_at.entry(vertex_key(*p)).or_default();
            *w = w.max(l.width);
        }
    }
    // Widest non-step line *ending* at each vertex: where a path ends at the stairs they
    // meet end to end, joined by a round landing.
    let mut end_width: std::collections::HashMap<(i32, i32), f32> = default();
    for l in lines.iter().filter(|l| l.kind != Kind::Steps) {
        for p in [l.points[0], *l.points.last().unwrap()] {
            let w = end_width.entry(vertex_key(p)).or_default();
            *w = w.max(l.width);
        }
    }
    // Where paths meet steps, the stairs themselves form the join; a round cap would cover the bottom step.
    let step_ends: std::collections::HashSet<(i32, i32)> = lines
        .iter()
        .filter(|l| l.kind == Kind::Steps)
        .flat_map(|l| [vertex_key(l.points[0]), vertex_key(*l.points.last().unwrap())])
        .collect();

    // One mesh per (chunk, look).
    let mut builders: std::collections::HashMap<((i32, i32), Look), Ribbons> = default();
    for line in &lines {
        let Some(&first) = line.points.first() else { continue };
        let key = ((first.x / CHUNK).floor() as i32, (first.y / CHUNK).floor() as i32);
        let lift = line.kind.lift();
        let line_look = look(line);
        if line.kind == Kind::Steps && !line.bridge {
            let trim = |p: &Vec2| width_at.get(&vertex_key(*p)).map_or(0.0, |w| w / 2.0);
            let pad = |p: &Vec2| {
                (width_at.get(&vertex_key(*p)).is_none())
                    .then(|| end_width.get(&vertex_key(*p)).map(|w| w.max(line.width.max(1.2)) / 2.0 + 0.15))
                    .flatten()
            };
            let pads = [pad(&line.points[0]), pad(line.points.last().unwrap())];
            let points = trim_polyline(&line.points, trim(&line.points[0]), trim(line.points.last().unwrap()));
            builders.entry((key, Look::Stone)).or_default().stairs(&points, line.width, &heightmap, pads);
            continue;
        }
        // Paths on a landmark terrace (the pergola) follow its deck; bridges get a straight
        // deck between their two ends (never below the terrain), plus structure.
        let mid = line.points[line.points.len() / 2];
        let on_terrace = landmarks.pergola_at(mid).is_some();
        let deck = (line.bridge && !on_terrace).then(|| {
            let (a, b) = (line.points[0], *line.points.last().unwrap());
            (heightmap.sample(a.x, a.y) + lift, heightmap.sample(b.x, b.y) + lift)
        });
        let profile = match deck {
            Some((h0, h1)) => Profile::Straight(h0, h1),
            None if on_terrace => Profile::Deck,
            None => Profile::Terrain,
        };
        // Square caps (extending the ends by half the width) fill the wedges at road
        // junctions, where overlapping asphalt looks seamless. Paths get round caps
        // instead, so they join steps and paths of other surfaces without notches.
        let caps = line_look == Look::Asphalt;
        builders.entry((key, line_look)).or_default().add(&line.points, line.width, lift, line.kind.repeat(), caps, profile, &heightmap);
        if !caps && !line.bridge && !on_terrace {
            let ribbons = builders.entry((key, line_look)).or_default();
            let n = line.points.len();
            let length: f32 = line.points.windows(2).map(|w| w[0].distance(w[1])).sum();
            let repeat = line.kind.repeat();
            // (end point, direction the strip runs at that end, texture v there)
            let ends = [
                (line.points[0], line.points[1] - line.points[0], 0.0),
                (line.points[n - 1], line.points[n - 1] - line.points[n - 2], length / repeat),
            ];
            for (end, dir, v0) in ends {
                // At stairs, the cap is a little wider so it covers the joint whatever the angle.
                let radius = if step_ends.contains(&vertex_key(end)) { line.width.max(1.2) / 2.0 + 0.3 } else { line.width / 2.0 };
                ribbons.disc(end, radius, lift, &heightmap, dir, line.width, repeat, v0);
            }
        }
        if let Some((h0, h1)) = deck {
            builders.entry((key, Look::Masonry)).or_default().bridge(&line.points, line.width, h0, h1, &heightmap);
            let total = line.points.windows(2).map(|w| w[0].distance(w[1])).sum();
            decks.0.push(Deck { points: line.points.clone(), half_width: line.width.max(2.0) / 2.0, h0, h1, total });
        }
        if has_centre_line(line) {
            let ribbons = builders.entry((key, Look::CentreLine)).or_default();
            for run in marking_runs(&line.points, &junctions) {
                ribbons.add(&run, CENTRE_LINE_WIDTH, lift + 0.02, line.kind.repeat(), false, Profile::Terrain, &heightmap);
            }
        }
    }
    // Round asphalt patches at junctions fill the slivers left between roads meeting at sharp angles.
    for (centre, width) in junctions.values() {
        let key = ((centre.x / CHUNK).floor() as i32, (centre.y / CHUNK).floor() as i32);
        builders.entry((key, Look::Asphalt)).or_default().disc(*centre, width / 2.0, Kind::RoadMajor.lift(), &heightmap, Vec2::X, *width, Kind::RoadMajor.repeat(), 0.0);
    }
    let root = commands.spawn((Lines, Name::new("Roads and paths"), Transform::default(), Visibility::default())).id();
    let mut triangles = 0;
    let mut per_chunk: std::collections::HashMap<(i32, i32), Vec<Entity>> = default();
    for ((key, l), ribbons) in builders {
        triangles += ribbons.indices.len() / 3;
        let mut e = commands.spawn((Mesh3d(meshes.add(ribbons.build())), MeshMaterial3d(material_for[&l].clone())));
        if !l.casts_shadows() {
            e.insert(NotShadowCaster);
        }
        let e = e.id();
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

/// The polyline with `start` metres cut off its beginning and `end` metres off its end
/// (keeping at least a short piece).
fn trim_polyline(points: &[Vec2], start: f32, end: f32) -> Vec<Vec2> {
    let total: f32 = points.windows(2).map(|w| w[0].distance(w[1])).sum();
    let (start, end) = if start + end > total - 0.8 {
        let scale = ((total - 0.8) / (start + end).max(1e-3)).max(0.0);
        (start * scale, end * scale)
    } else {
        (start, end)
    };
    let (from, to) = (start, total - end);
    let mut out = Vec::new();
    let mut along = 0.0;
    for w in points.windows(2) {
        let len = w[0].distance(w[1]);
        let (a, b) = (along, along + len);
        if b >= from && a <= to {
            let t0 = ((from - a) / len.max(1e-6)).clamp(0.0, 1.0);
            let t1 = ((to - a) / len.max(1e-6)).clamp(0.0, 1.0);
            if out.is_empty() {
                out.push(w[0].lerp(w[1], t0));
            }
            out.push(w[0].lerp(w[1], t1));
        }
        along = b;
    }
    if out.len() < 2 { points.to_vec() } else { out }
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

/// How a strip's height is found.
#[derive(Clone, Copy)]
enum Profile {
    /// Draped on the terrain.
    Terrain,
    /// A straight bridge deck from the first height to the second.
    Straight(f32, f32),
    /// On a landmark terrace (the pergola deck).
    Deck,
}

#[derive(Default)]
struct Ribbons {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    /// Only filled by `quad` (3D stairs and bridges, whose meshes use nothing else).
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

impl Ribbons {
    /// A strip along `points`, following the terrain, a straight bridge deck, or a
    /// landmark terrace deck (see `Profile`).
    #[allow(clippy::too_many_arguments)]
    fn add(&mut self, points: &[Vec2], width: f32, lift: f32, repeat: f32, caps: bool, profile: Profile, hm: &Heightmap) {
        // Not for bridge decks (in the air) or centre lines.
        let skirts = !matches!(profile, Profile::Straight(..)) && width > 0.5;
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
        let cols = ((width / ACROSS_SPACING).ceil() as usize + 1).max(2);
        let total: f32 = pts.windows(2).map(|w| w[0].distance(w[1])).sum();
        let height = |p: Vec2, along: f32| match profile {
            Profile::Straight(h0, h1) => (h0 + (h1 - h0) * (along / total.max(1e-3))).max(hm.sample(p.x, p.y) + lift),
            Profile::Deck => deck_height(hm, p) + lift,
            Profile::Terrain => hm.sample(p.x, p.y) + lift,
        };
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
            // Vertices across the width too, so the strip follows the terrain sideways
            // (road camber, terrain triangle folds) instead of letting it poke through.
            for k in 0..cols {
                let u = k as f32 / (cols - 1) as f32;
                let p = pts[i] + normal * (half * mitre * (2.0 * u - 1.0));
                self.positions.push([p.x, height(p, along), p.y]);
                self.normals.push([0.0, 1.0, 0.0]);
                self.uvs.push([u, along / repeat]);
            }
        }
        // Skirts: both long edges continue straight down into the ground, so the small lift
        // never shows as a gap (or a dark seam) between the strip and the terrain.
        let rows = pts.len() as u32;
        let cols32 = cols as u32;
        if skirts {
            for edge_col in [0, cols32 - 1] {
                let base = self.positions.len() as u32;
                for i in 0..rows {
                    let top = self.positions[(start + i * cols32 + edge_col) as usize];
                    let uv = self.uvs[(start + i * cols32 + edge_col) as usize];
                    self.positions.push([top[0], top[1] - lift - SKIRT_DEPTH, top[2]]);
                    self.normals.push([0.0, 1.0, 0.0]);
                    self.uvs.push(uv);
                }
                for i in 0..rows - 1 {
                    let (t0, t1) = (start + i * cols32 + edge_col, start + (i + 1) * cols32 + edge_col);
                    let (b0, b1) = (base + i, base + i + 1);
                    // Both windings: skirts are seen from outside only, but which side is
                    // "outside" depends on the line's direction; two triangles each way is cheap.
                    self.indices.extend_from_slice(&[t0, b0, t1, t1, b0, b1, t0, t1, b0, t1, b1, b0]);
                }
            }
        }
        for i in 0..pts.len() as u32 - 1 {
            for k in 0..cols32 - 1 {
                let (l0, r0) = (start + i * cols32 + k, start + i * cols32 + k + 1);
                let (l1, r1) = (l0 + cols32, r0 + cols32);
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
    }

    /// A flat disc draped on the terrain (a fan of triangles facing up), textured as a
    /// continuation of a strip of `width` running along `dir` whose texture is at `v0` at
    /// the centre — so a path's round end cap shows the path's own surface pattern.
    #[allow(clippy::too_many_arguments)]
    fn disc(&mut self, centre: Vec2, radius: f32, lift: f32, hm: &Heightmap, dir: Vec2, width: f32, repeat: f32, v0: f32) {
        const SIDES: usize = 16;
        let dir = dir.normalize_or(Vec2::X);
        let across = Vec2::new(-dir.y, dir.x);
        let uv = |p: Vec2| {
            let d = p - centre;
            [(0.5 + d.dot(across) / width.max(0.1)).clamp(0.0, 1.0), v0 + d.dot(dir) / repeat]
        };
        let start = self.positions.len() as u32;
        let push = |p: Vec2, this: &mut Self| {
            this.positions.push([p.x, hm.sample(p.x, p.y) + lift, p.y]);
            this.normals.push([0.0, 1.0, 0.0]);
            this.uvs.push(uv(p));
        };
        push(centre, self);
        for ring in [1.0, 0.5] {
            for k in 0..SIDES {
                let a = k as f32 / SIDES as f32 * std::f32::consts::TAU;
                push(centre + Vec2::new(a.cos(), a.sin()) * radius * ring, self);
            }
        }
        let (outer, inner) = (start + 1, start + 1 + SIDES as u32);
        for k in 0..SIDES as u32 {
            // Angles increase from +x towards +z (south), i.e. clockwise seen from above, so
            // (centre, next, current) is counter-clockwise from above and faces up.
            let next = (k + 1) % SIDES as u32;
            self.indices.extend_from_slice(&[start, inner + next, inner + k]);
            self.indices.extend_from_slice(&[inner + k, inner + next, outer + next]);
            self.indices.extend_from_slice(&[inner + k, outer + next, outer + k]);
        }
    }

    /// A quad a→b→c→d (counter-clockwise when seen from the side it faces), flat shaded,
    /// with world-scale texture coordinates (one repeat per 2 m).
    fn quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3) {
        let n = (b - a).cross(d - a).normalize_or_zero();
        if n == Vec3::ZERO {
            return;
        }
        let i = self.positions.len() as u32;
        let (w, h) = (a.distance(b) / 2.0, a.distance(d) / 2.0);
        // Baked shading: vertical faces (stair risers, walls) darker, undersides darker still,
        // so steps read clearly even when the sun lights risers and treads alike.
        let shade = if n.y.abs() < 0.5 { 0.68 } else if n.y < 0.0 { 0.55 } else { 1.0 };
        for (p, uv) in [(a, [0.0, 0.0]), (b, [w, 0.0]), (c, [w, h]), (d, [0.0, h])] {
            self.positions.push(p.to_array());
            self.normals.push(n.to_array());
            self.uvs.push(uv);
            self.colors.push([shade, shade, shade, 1.0]);
        }
        self.indices.extend_from_slice(&[i, i + 1, i + 2, i, i + 2, i + 3]);
    }

    /// Box from its four bottom corners (counter-clockwise from above) up to `top`.
    fn prism(&mut self, base: [Vec2; 4], bottom: f32, top: f32) {
        let at = |p: Vec2, y: f32| Vec3::new(p.x, y, p.y);
        // Make sure the corners run counter-clockwise when seen from above (+y; z points south).
        let ccw = {
            let mut a = 0.0;
            for k in 0..4 {
                a += base[k].perp_dot(base[(k + 1) % 4]);
            }
            a < 0.0
        };
        let c = if ccw { base } else { [base[3], base[2], base[1], base[0]] };
        for k in 0..4 {
            let (p, q) = (c[k], c[(k + 1) % 4]);
            self.quad(at(q, bottom), at(p, bottom), at(p, top), at(q, top));
        }
        self.quad(at(c[0], top), at(c[3], top), at(c[2], top), at(c[1], top));
    }

    /// Stairs along a steps line: treads about 0.4 m deep, each step's top at the terrain
    /// height rounded to 16 cm rises (so the steps follow the slope as it really is, with
    /// landings where it flattens), each tread a box down into the ground. A short landing
    /// at each end overlaps the connecting path, so stairs and paths join without gaps.
    fn stairs(&mut self, points: &[Vec2], width: f32, hm: &Heightmap, pads: [Option<f32>; 2]) {
        const RISE: f32 = 0.16;
        const TREAD: f32 = 0.4;
        const LANDING: f32 = 0.6;
        let half = width.max(1.2) / 2.0;
        let mut along = vec![0.0f32];
        for w in points.windows(2) {
            along.push(along.last().unwrap() + w[0].distance(w[1]));
        }
        let total = *along.last().unwrap();
        if total < 0.5 {
            return;
        }
        // Positions along the line, extended straight past both ends for the landings. The
        // extension direction comes from a point at least 1 m back, so a tiny final segment
        // (common where the steps meet a path) can't swing a landing sideways.
        let n_pts = points.len();
        let back = |from_end: bool| {
            let anchor = if from_end { points[n_pts - 1] } else { points[0] };
            let iter: Box<dyn Iterator<Item = &Vec2>> =
                if from_end { Box::new(points.iter().rev()) } else { Box::new(points.iter()) };
            let far = iter.skip(1).find(|p| p.distance(anchor) >= 1.0).copied().unwrap_or(if from_end { points[0] } else { points[n_pts - 1] });
            (anchor - far).normalize_or(Vec2::X)
        };
        let (start_dir, end_dir) = (back(false), back(true));
        let point_at = |d: f32| {
            if d < 0.0 {
                return points[0] + start_dir * -d;
            }
            if d > total {
                return points[n_pts - 1] + end_dir * (d - total);
            }
            let k = along.partition_point(|&a| a <= d).clamp(1, n_pts - 1);
            let t = (d - along[k - 1]) / (along[k] - along[k - 1]).max(1e-6);
            points[k - 1].lerp(points[k], t.clamp(0.0, 1.0))
        };
        let h0 = hm.sample(points[0].x, points[0].y);
        let n = (total / TREAD).round().max(1.0) as usize;
        let tread = total / n as f32;
        let mut treads: Vec<(f32, f32, bool)> = Vec::new();
        if pads[0].is_none() {
            treads.push((-LANDING, 0.0, true));
        }
        treads.extend((0..n).map(|k| (k as f32 * tread, (k + 1) as f32 * tread, false)));
        if pads[1].is_none() {
            treads.push((total, total + LANDING, true));
        }
        // Where a path ends at the stairs, the path's own round end cap (drawn with the
        // path) fills the join at any angle; the stairs start on top of it.
        for (d0, d1, landing) in treads {
            let (p0, p1) = (point_at(d0), point_at(d1));
            let mid = point_at((d0 + d1) / 2.0);
            // Direction over a longer stretch, so a tiny kink in the line can't twist a step.
            let dir = (point_at((d1 + 0.8).min(total + LANDING)) - point_at((d0 - 0.8).max(-LANDING))).normalize_or(Vec2::X);
            let side = Vec2::new(-dir.y, dir.x) * half;
            let (q0, q1) = (mid - dir * (d1 - d0) / 2.0, mid + dir * (d1 - d0) / 2.0);
            // Highest terrain under this tread (centre and both side edges), rounded up to a whole rise.
            let ground_top = [mid, mid + side, mid - side, p0, p1].iter().map(|p| hm.sample(p.x, p.y)).fold(f32::MIN, f32::max);
            // Landings sit flush with the path surface they join (paths float ~0.06–0.08 m up).
            let top = if landing {
                ground_top + 0.09
            } else {
                // Always clear of path surfaces and end caps (~0.06–0.08 m up).
                (h0 + ((ground_top - h0) / RISE).ceil() * RISE + 0.04).max(ground_top + 0.11)
            };
            let ground = [p0, p1, p0 + side, p1 - side].iter().map(|p| hm.sample(p.x, p.y)).fold(f32::MAX, f32::min);
            self.prism([q0 - side, q1 - side, q1 + side, q0 + side], ground - 0.4, top);
        }
    }

    /// Bridge structure under a deck running straight from height `h0` to `h1`:
    /// a slab, parapet walls and piers down to the ground.
    fn bridge(&mut self, points: &[Vec2], width: f32, h0: f32, h1: f32, hm: &Heightmap) {
        const SLAB: f32 = 0.7;
        const PARAPET: f32 = 1.0;
        const WALL: f32 = 0.35;
        const PIER_SPACING: f32 = 9.0;
        let half = width.max(2.0) / 2.0;
        let total: f32 = points.windows(2).map(|w| w[0].distance(w[1])).sum();
        let mut along = 0.0;
        let mut next_pier = PIER_SPACING / 2.0;
        let mut first = true;
        let segments = points.len() - 1;
        for (seg, w) in points.windows(2).enumerate() {
            let last = seg + 1 == segments;
            let (a, b) = (w[0], w[1]);
            let len = a.distance(b);
            let n = (len / MAX_SEGMENT).ceil().max(1.0) as usize;
            let dir = (b - a).normalize_or(Vec2::X);
            let side = Vec2::new(-dir.y, dir.x);
            for k in 0..n {
                let (t0, t1) = (k as f32 / n as f32, (k + 1) as f32 / n as f32);
                let (p0, p1) = (a.lerp(b, t0), a.lerp(b, t1));
                let deck = |d: f32, p: Vec2| (h0 + (h1 - h0) * (d / total.max(1e-3))).max(hm.sample(p.x, p.y));
                let (d0, d1) = (along + len * t0, along + len * t1);
                let (y0, y1) = (deck(d0, p0), deck(d1, p1));
                for s in [-1.0f32, 1.0] {
                    // Parapet wall along this edge, from under the slab to above the deck.
                    let (i0, i1) = (p0 + side * s * (half - WALL), p1 + side * s * (half - WALL));
                    let (o0, o1) = (p0 + side * s * half, p1 + side * s * half);
                    let bottom = y0.min(y1) - SLAB;
                    let top0 = y0 + PARAPET;
                    let top1 = y1 + PARAPET;
                    let at = |p: Vec2, y: f32| Vec3::new(p.x, y, p.y);
                    // Outer face, top, inner face (wound to face out / up / in for either side).
                    let (oa, ob) = if s > 0.0 { (o1, o0) } else { (o0, o1) };
                    let (ta, tb) = if s > 0.0 { (top1, top0) } else { (top0, top1) };
                    self.quad(at(oa, bottom), at(ob, bottom), at(ob, tb), at(oa, ta));
                    let (ia, ib) = if s > 0.0 { (i0, i1) } else { (i1, i0) };
                    let (tia, tib) = if s > 0.0 { (top0, top1) } else { (top1, top0) };
                    self.quad(at(ia, y0.min(y1)), at(ib, y0.min(y1)), at(ib, tib), at(ia, tia));
                    let (q0, q1) = if s > 0.0 { (o0, o1) } else { (o1, o0) };
                    let (r0, r1) = if s > 0.0 { (i0, i1) } else { (i1, i0) };
                    let (y_a, y_b) = if s > 0.0 { (top0, top1) } else { (top1, top0) };
                    self.quad(at(q0, y_a), at(q1, y_b), at(r1, y_b), at(r0, y_a));
                    // Close the wall at the bridge's two ends.
                    if first {
                        self.quad(at(o0, bottom), at(i0, bottom), at(i0, top0), at(o0, top0));
                    }
                    if last && k == n - 1 {
                        self.quad(at(i1, bottom), at(o1, bottom), at(o1, top1), at(i1, top1));
                    }
                }
                // Slab underside, facing down, and its ends.
                let at = |p: Vec2, y: f32| Vec3::new(p.x, y, p.y);
                let (l0, l1, r0, r1) = (p0 + side * half, p1 + side * half, p0 - side * half, p1 - side * half);
                self.quad(at(r0, y0 - SLAB), at(r1, y1 - SLAB), at(l1, y1 - SLAB), at(l0, y0 - SLAB));
                if first {
                    self.quad(at(l0, y0 - SLAB), at(r0, y0 - SLAB), at(r0, y0), at(l0, y0));
                }
                if last && k == n - 1 {
                    self.quad(at(r1, y1 - SLAB), at(l1, y1 - SLAB), at(l1, y1), at(r1, y1));
                }
                first = false;
                // Piers where the deck is well above the ground.
                while next_pier <= d1 {
                    let p = a.lerp(b, ((next_pier - along) / len).clamp(0.0, 1.0));
                    let y = deck(next_pier, p);
                    let ground = hm.sample(p.x, p.y);
                    if y - SLAB - ground > 1.5 {
                        let (f, s2) = (dir * 0.6, side * (half - 0.2));
                        self.prism([p - f - s2, p + f - s2, p + f + s2, p - f + s2], ground - 1.0, y - SLAB);
                    }
                    next_pier += PIER_SPACING;
                }
            }
            along += len;
        }
    }

    fn build(self) -> Mesh {
        let with_colors = !self.colors.is_empty() && self.colors.len() == self.positions.len();
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
            .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals)
            .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs)
            .with_inserted_indices(Indices::U32(self.indices));
        if with_colors {
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, self.colors);
        }
        mesh
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
                    let s = 0.66 + 0.05 * n;
                    (s, s, s * 0.97)
                }
                Look::Masonry => {
                    // Stone blocks: mortar lines every 1/4 vertically, staggered every 1/2 across.
                    let row = (v * 4.0).floor();
                    let offset = if row as i32 % 2 == 0 { 0.0 } else { 0.25 };
                    let joint = (v * 4.0).fract() < 0.06 || ((u + offset) * 2.0).fract() < 0.03;
                    let s = if joint { 0.55 } else { 0.72 + 0.06 * n };
                    (s, s * 0.95, s * 0.86)
                }
            };
            // Centre line: 6 m dash, 6 m gap (one repeat is 12 m).
            let alpha = if look == Look::CentreLine && v >= 0.5 { 0 } else { 255 };
            let i = (y * W + x) * 4;
            let to8 = |c: f32| (c.clamp(0.0, 1.0) * 255.0) as u8;
            rgba[i..i + 4].copy_from_slice(&[to8(r), to8(g), to8(b), alpha]);
        }
    }
    // 3D surfaces use world-scale coordinates and tile both ways; strips repeat only along.
    let address_u = if matches!(look, Look::Stone | Look::Masonry) { ImageAddressMode::Repeat } else { ImageAddressMode::ClampToEdge };
    image_with_mips(W, H, rgba, TextureFormat::Rgba8UnormSrgb, address_u, ImageAddressMode::Repeat)
}
