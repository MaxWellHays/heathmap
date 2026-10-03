//! Roads and paths as ribbon meshes draped on the terrain, so they stay sharp at any
//! distance (unlike the ground texture). Each ribbon follows the terrain mesh exactly
//! (heights sampled every few metres) and sits slightly above it; materials use
//! procedurally generated, mipmapped, anisotropically filtered textures — asphalt
//! with dashed centre lines for two-way roads, paving, gravel and dirt for paths.

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::buildings::Reader;
use crate::level::{BinaryFile, Heightmap, LevelHandles, LevelState};
use crate::lod::LodChunk;

const CHUNK: f32 = 512.0;
const MAX_SEGMENT: f32 = 3.0; // m; ribbons are resampled this finely to follow the terrain
const DRAW_DISTANCE: [f32; 1] = [3000.0];

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
    AsphaltCentreLine,
    AsphaltPlain,
    Paving,
    Gravel,
    Dirt,
    Stone,
}

fn look(line: &Line) -> Look {
    match line.kind {
        Kind::RoadMajor | Kind::RoadMinor if !line.oneway && line.lanes != 1 => Look::AsphaltCentreLine,
        Kind::RoadMajor | Kind::RoadMinor | Kind::Service => Look::AsphaltPlain,
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
    for l in [Look::AsphaltCentreLine, Look::AsphaltPlain, Look::Paving, Look::Gravel, Look::Dirt, Look::Stone] {
        let image = images.add(texture(l));
        let m = materials.add(StandardMaterial {
            base_color_texture: Some(image),
            perceptual_roughness: 0.95,
            reflectance: 0.1,
            depth_bias: 50.0,
            ..default()
        });
        material_for.insert(l, m);
    }

    // One mesh per (chunk, look).
    let mut builders: std::collections::HashMap<((i32, i32), Look), Ribbons> = default();
    for line in &lines {
        let Some(&first) = line.points.first() else { continue };
        let key = ((first.x / CHUNK).floor() as i32, (first.y / CHUNK).floor() as i32);
        builders.entry((key, look(line))).or_default().add(line, &heightmap);
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

#[derive(Default)]
struct Ribbons {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl Ribbons {
    fn add(&mut self, line: &Line, hm: &Heightmap) {
        // Resample so the ribbon follows the terrain between OSM vertices.
        let mut pts = Vec::with_capacity(line.points.len() * 2);
        for w in line.points.windows(2) {
            let n = ((w[1] - w[0]).length() / MAX_SEGMENT).ceil().max(1.0) as usize;
            for k in 0..n {
                pts.push(w[0].lerp(w[1], k as f32 / n as f32));
            }
        }
        pts.push(*line.points.last().unwrap());
        pts.dedup_by(|a, b| a.distance_squared(*b) < 1e-4);
        if pts.len() < 2 {
            return;
        }
        let half = line.width / 2.0;
        let lift = line.kind.lift();
        let repeat = line.kind.repeat();
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
            let (mut r, mut g, mut b) = match look {
                Look::AsphaltCentreLine | Look::AsphaltPlain => (0.30 + 0.05 * n, 0.30 + 0.05 * n, 0.32 + 0.05 * n),
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
            if look == Look::AsphaltCentreLine && (u - 0.5).abs() < 0.035 && v < 0.5 {
                (r, g, b) = (0.92, 0.92, 0.90); // dashed centre line
            }
            if matches!(look, Look::AsphaltCentreLine | Look::AsphaltPlain) && (u < 0.03 || u > 0.97) {
                (r, g, b) = (0.55, 0.55, 0.55); // kerb edge
            }
            let i = (y * W + x) * 4;
            let to8 = |c: f32| (c.clamp(0.0, 1.0) * 255.0) as u8;
            rgba[i..i + 4].copy_from_slice(&[to8(r), to8(g), to8(b), 255]);
        }
    }
    image_with_mips(W, H, rgba)
}

/// RGBA8 sRGB image with a full box-filtered mip chain, repeating, anisotropic sampling —
/// keeps markings crisp up close and stops shimmering in the distance.
fn image_with_mips(w: usize, h: usize, base: Vec<u8>) -> Image {
    let mut data = base.clone();
    let (mut lw, mut lh, mut level) = (w, h, base);
    let mut levels = 1;
    while lw > 1 || lh > 1 {
        let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
        let mut next = vec![0u8; nw * nh * 4];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let mut sum = 0u32;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let (sx, sy) = ((2 * x + dx).min(lw - 1), (2 * y + dy).min(lh - 1));
                        sum += level[(sy * lw + sx) * 4 + c] as u32;
                    }
                    next[(y * nw + x) * 4 + c] = (sum / 4) as u8;
                }
            }
        }
        data.extend_from_slice(&next);
        (lw, lh, level) = (nw, nh, next);
        levels += 1;
    }
    // `Image::new` only accepts the base level; mip levels follow it in `data`.
    let mut image = Image::new_uninit(
        Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = levels;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 16,
        ..default()
    });
    image
}
