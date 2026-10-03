//! Water: ponds and lakes as flat surfaces at their water level, with a glossy material
//! and a slowly drifting ripple normal map for moving sun glints.
//!
//! The LIDAR terrain over water is an interpolated, roughly flat surface at about the
//! water level, so the level is taken as a high percentile of the terrain inside each
//! outline: the surface then covers the whole pond rather than leaving terrain showing
//! through it.

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageAddressMode;
use bevy::light::NotShadowCaster;
use bevy::math::Affine2;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;

use crate::buildings::Reader;
use crate::level::{BinaryFile, Heightmap, LevelHandles, LevelState};
use crate::textures::image_with_mips;

const LEVEL_PERCENTILE: f32 = 0.85;
const LEVEL_OFFSET: f32 = 0.05; // m above that percentile
const RIPPLE_SCALE: f32 = 12.0; // m per normal-map repeat

#[derive(Component)]
pub struct Water;

#[derive(Resource)]
struct WaterMaterial(Handle<StandardMaterial>);

pub struct WaterPlugin;

impl Plugin for WaterPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(LevelState::Ready), spawn_water)
            .add_systems(Update, drift_ripples.run_if(resource_exists::<WaterMaterial>));
    }
}

fn decode(bytes: &[u8]) -> Vec<Vec<Vec<Vec2>>> {
    let mut r = Reader { bytes, pos: 0 };
    (0..r.u32())
        .map(|_| (0..r.u16()).map(|_| (0..r.u32()).map(|_| Vec2::new(r.f32(), r.f32())).collect()).collect())
        .collect()
}

fn spawn_water(
    mut commands: Commands,
    handles: Res<LevelHandles>,
    files: Res<Assets<BinaryFile>>,
    hm: Res<Heightmap>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(file) = handles.water.as_ref().and_then(|h| files.get(h)) else { return };
    let polygons = decode(&file.0);
    let material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.16, 0.30, 0.36),
        perceptual_roughness: 0.12,
        reflectance: 0.6,
        normal_map_texture: Some(images.add(ripple_normal_map())),
        ..default()
    });
    commands.insert_resource(WaterMaterial(material.clone()));

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    for rings in &polygons {
        let Some(level) = water_level(rings, &hm) else { continue };
        let flat: Vec<f64> = rings.iter().flatten().flat_map(|p| [p.x as f64, p.y as f64]).collect();
        let mut holes = Vec::new();
        let mut n = 0;
        for (i, ring) in rings.iter().enumerate() {
            if i > 0 {
                holes.push(n);
            }
            n += ring.len();
        }
        let Ok(tris) = earcutr::earcut(&flat, &holes, 2) else { continue };
        let base = positions.len() as u32;
        positions.extend(rings.iter().flatten().map(|p| [p.x, level, p.y]));
        for t in tris.chunks_exact(3) {
            let (a, b, c) = (t[0], t[1], t[2]);
            // Face up: counter-clockwise seen from above (+y; z points south).
            let (pa, pb, pc) = (positions[base as usize + a], positions[base as usize + b], positions[base as usize + c]);
            let up = (pb[0] - pa[0]) * (pc[2] - pa[2]) - (pb[2] - pa[2]) * (pc[0] - pa[0]) < 0.0;
            let (b, c) = if up { (b, c) } else { (c, b) };
            indices.extend_from_slice(&[base + a as u32, base + b as u32, base + c as u32]);
        }
    }
    let count = positions.len();
    let uvs: Vec<[f32; 2]> = positions.iter().map(|p| [p[0] / RIPPLE_SCALE, p[2] / RIPPLE_SCALE]).collect();
    let mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; count])
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_attribute(Mesh::ATTRIBUTE_TANGENT, vec![[1.0, 0.0, 0.0, 1.0]; count])
        .with_inserted_indices(Indices::U32(indices));
    commands.spawn((Water, Name::new("Water"), Mesh3d(meshes.add(mesh)), MeshMaterial3d(material), NotShadowCaster));
    info!("Spawned {} water surfaces", polygons.len());
}

/// Water level: a high percentile of terrain heights sampled inside the outline.
fn water_level(rings: &[Vec<Vec2>], hm: &Heightmap) -> Option<f32> {
    let outer = rings.first()?;
    let (lo, hi) = outer.iter().fold((Vec2::MAX, Vec2::MIN), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    let step = ((hi - lo).max_element() / 30.0).clamp(2.0, 8.0);
    let mut samples = Vec::new();
    let mut y = lo.y;
    while y <= hi.y {
        let mut x = lo.x;
        while x <= hi.x {
            let p = Vec2::new(x, y);
            if inside(p, outer) && !rings[1..].iter().any(|h| inside(p, h)) {
                samples.push(hm.sample(x, y));
            }
            x += step;
        }
        y += step;
    }
    if samples.is_empty() {
        samples = outer.iter().map(|p| hm.sample(p.x, p.y)).collect();
    }
    samples.sort_by(f32::total_cmp);
    Some(samples[((samples.len() - 1) as f32 * LEVEL_PERCENTILE) as usize] + LEVEL_OFFSET)
}

fn inside(p: Vec2, ring: &[Vec2]) -> bool {
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

/// Tileable ripple normal map from a few crossing sine waves (whole numbers of waves per tile).
fn ripple_normal_map() -> Image {
    const N: usize = 256;
    let waves: [(f32, f32, f32, f32); 5] = [(3.0, 1.0, 0.30, 0.0), (-2.0, 4.0, 0.22, 1.3), (5.0, -3.0, 0.15, 2.1), (1.0, 7.0, 0.10, 0.7), (-8.0, -5.0, 0.06, 4.0)];
    let mut rgba = vec![0u8; N * N * 4];
    for y in 0..N {
        for x in 0..N {
            let (u, v) = (x as f32 / N as f32, y as f32 / N as f32);
            let (mut dx, mut dy) = (0.0, 0.0);
            for (kx, ky, amp, phase) in waves {
                let arg = std::f32::consts::TAU * (kx * u + ky * v) + phase;
                // Height = amp·sin(arg); gradient scaled to give gentle slopes.
                dx += amp * kx * arg.cos() * 0.04;
                dy += amp * ky * arg.cos() * 0.04;
            }
            let n = Vec3::new(-dx, -dy, 1.0).normalize();
            let i = (y * N + x) * 4;
            let enc = |c: f32| ((c * 0.5 + 0.5) * 255.0).round() as u8;
            rgba[i..i + 4].copy_from_slice(&[enc(n.x), enc(n.y), enc(n.z), 255]);
        }
    }
    image_with_mips(N, N, rgba, TextureFormat::Rgba8Unorm, ImageAddressMode::Repeat, ImageAddressMode::Repeat)
}

fn drift_ripples(time: Res<Time>, water: Res<WaterMaterial>, mut materials: ResMut<Assets<StandardMaterial>>) {
    if let Some(mut m) = materials.get_mut(&water.0) {
        let t = time.elapsed_secs();
        m.uv_transform = Affine2::from_translation(Vec2::new(t * 0.010, t * 0.006));
    }
}
