//! Trees: every LIDAR-detected tree as a low-poly shape, merged into one mesh per
//! chunk (a few hundred draw calls, frustum-culled per chunk). Each chunk has three
//! levels of detail:
//!   0 (near):   hexagonal bipyramid crown on a triangular trunk — 18 triangles
//!   1 (middle): square bipyramid crown, no trunk                 —  8 triangles
//!   2 (far):    triangular pyramid crown top                     —  3 triangles
//! Trees stand on the terrain as rendered (heights sampled from the terrain mesh),
//! with trunks sunk a little into the ground so slopes never show a gap.

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use crate::level::{Heightmap, Level, Tree, spawn_step};
use crate::lod::LodChunk;

const CHUNK: f32 = 256.0;
const LOD_DISTANCES: [f32; 2] = [450.0, 1400.0];
const TRUNK_SINK: f32 = 0.6;

#[derive(Component)]
pub struct Trees;

pub struct TreesPlugin;

impl Plugin for TreesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, spawn_trees.run_if(spawn_step(2)));
    }
}

#[derive(Clone, Copy)]
struct TreeShape {
    crown_sides: usize,
    crown_bottom: bool,
    trunk_sides: usize,
}

const SHAPES: [TreeShape; 3] = [
    TreeShape { crown_sides: 6, crown_bottom: true, trunk_sides: 3 },
    TreeShape { crown_sides: 4, crown_bottom: true, trunk_sides: 0 },
    TreeShape { crown_sides: 3, crown_bottom: false, trunk_sides: 0 },
];

fn spawn_trees(
    mut commands: Commands,
    level: Res<Level>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let material = materials.add(StandardMaterial { perceptual_roughness: 0.9, reflectance: 0.05, ..default() });
    let mut chunks: std::collections::HashMap<(i32, i32), Vec<Tree>> = default();
    for t in &level.trees {
        let key = ((t.x / CHUNK).floor() as i32, (t.z / CHUNK).floor() as i32);
        chunks.entry(key).or_default().push(*t);
    }
    let root = commands.spawn((Trees, Transform::default(), Visibility::default(), Name::new("Trees"))).id();
    let mut triangles = [0usize; 3];
    for trees in chunks.values() {
        let center = {
            let (sx, sz) = trees.iter().fold((0.0, 0.0), |(x, z), t| (x + t.x, z + t.z));
            let (x, z) = (sx / trees.len() as f32, sz / trees.len() as f32);
            Vec3::new(x, level.heightmap.sample(x, z), z)
        };
        let levels: Vec<Entity> = SHAPES
            .iter()
            .enumerate()
            .map(|(i, shape)| {
                let mesh = trees_mesh(trees, &level.heightmap, *shape);
                triangles[i] += mesh.indices().map_or(0, |idx| idx.len() / 3);
                let mut e = commands.spawn((
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(material.clone()),
                    if i == 0 { Visibility::Inherited } else { Visibility::Hidden },
                ));
                if i == 2 {
                    e.insert(NotShadowCaster); // far away: shadows wouldn't be visible anyway
                }
                e.id()
            })
            .collect();
        let chunk = commands
            .spawn((LodChunk { center, levels: levels.clone(), distances: &LOD_DISTANCES }, Transform::default(), Visibility::default()))
            .add_children(&levels)
            .id();
        commands.entity(root).add_child(chunk);
    }
    info!(
        "Spawned {} trees in {} chunks; triangles per LOD: {triangles:?}",
        level.trees.len(),
        chunks.len()
    );
}

/// Cheap deterministic per-tree random number in [0, 1).
fn hash01(x: f32, z: f32) -> f32 {
    let h = (x * 12.9898 + z * 78.233).sin() * 43758.547;
    h - h.floor()
}

fn trees_mesh(trees: &[Tree], hm: &Heightmap, shape: TreeShape) -> Mesh {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let trunk_color = LinearRgba::from(Color::srgb(0.36, 0.27, 0.18)).to_f32_array();

    for t in trees {
        let base = Vec3::new(t.x, hm.sample(t.x, t.z), t.z);
        let h = t.height.max(3.0);
        let r = t.crown_radius.clamp(1.0, h * 0.6);
        let jitter = hash01(t.x, t.z);
        // Darker greens for taller trees; a little random variation per tree.
        let crown_color = LinearRgba::from(Color::srgb(
            0.20 + 0.10 * jitter - 0.004 * h,
            0.42 + 0.12 * jitter - 0.005 * h,
            0.16 + 0.05 * jitter,
        ))
        .to_f32_array();
        let crown_bottom = h * 0.25;
        let crown_mid = h * 0.55;

        if shape.trunk_sides > 0 {
            let n = shape.trunk_sides;
            let trunk_r = (0.08 * h).clamp(0.15, 0.6);
            let start = positions.len() as u32;
            for k in 0..n {
                let a = (k as f32 + 0.5 * jitter) / n as f32 * std::f32::consts::TAU;
                let d = Vec3::new(a.cos(), 0.0, a.sin());
                for y in [-TRUNK_SINK, crown_mid] {
                    positions.push((base + d * trunk_r + Vec3::Y * y).to_array());
                    normals.push(d.to_array());
                    colors.push(trunk_color);
                }
            }
            for k in 0..n as u32 {
                let (a0, a1) = (start + 2 * k, start + 2 * k + 1);
                let next = (k + 1) % n as u32;
                let (b0, b1) = (start + 2 * next, start + 2 * next + 1);
                indices.extend_from_slice(&[a0, a1, b0, b0, a1, b1]);
            }
        }

        // Crown: ring at crown_mid, tip at h, optional bottom tip at crown_bottom.
        let n = shape.crown_sides;
        let start = positions.len() as u32;
        for k in 0..n {
            let a = (k as f32 + jitter) / n as f32 * std::f32::consts::TAU;
            let d = Vec3::new(a.cos(), 0.0, a.sin());
            positions.push((base + d * r + Vec3::Y * crown_mid).to_array());
            normals.push(d.to_array());
            colors.push(crown_color);
        }
        let top = start + n as u32;
        positions.push((base + Vec3::Y * h).to_array());
        normals.push([0.0, 1.0, 0.0]);
        colors.push(crown_color);
        let bottom = top + 1;
        if shape.crown_bottom {
            positions.push((base + Vec3::Y * crown_bottom).to_array());
            normals.push([0.0, -1.0, 0.0]);
            colors.push(crown_color);
        }
        for k in 0..n as u32 {
            let (a, b) = (start + k, start + (k + 1) % n as u32);
            indices.extend_from_slice(&[a, top, b]);
            if shape.crown_bottom {
                indices.extend_from_slice(&[a, b, bottom]);
            }
        }
    }

    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
        .with_inserted_indices(Indices::U32(indices))
}
