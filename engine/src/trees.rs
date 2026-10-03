//! Trees: every LIDAR-detected tree as a low-poly shape (hexagonal bipyramid crown
//! on a triangular trunk), merged into one mesh per chunk so the whole forest is a
//! few hundred draw calls with per-chunk frustum culling.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use crate::level::{Level, LevelState, Tree};

const CHUNK: f32 = 512.0;
const CROWN_SIDES: usize = 6;
const TRUNK_SIDES: usize = 3;

#[derive(Component)]
pub struct Trees;

pub struct TreesPlugin;

impl Plugin for TreesPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(LevelState::Ready), spawn_trees);
    }
}

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
    let mut triangles = 0;
    for trees in chunks.values() {
        let mesh = trees_mesh(trees);
        triangles += mesh.indices().map_or(0, |i| i.len() / 3);
        let child = commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(material.clone()))).id();
        commands.entity(root).add_child(child);
    }
    info!("Spawned {} trees in {} chunks ({triangles} triangles)", level.trees.len(), chunks.len());
}

/// Cheap deterministic per-tree random number in [0, 1).
fn hash01(x: f32, z: f32) -> f32 {
    let h = (x * 12.9898 + z * 78.233).sin() * 43758.547;
    h - h.floor()
}

fn trees_mesh(trees: &[Tree]) -> Mesh {
    let per_tree_verts = CROWN_SIDES + 2 + TRUNK_SIDES * 2;
    let mut positions = Vec::with_capacity(trees.len() * per_tree_verts);
    let mut normals = Vec::with_capacity(positions.capacity());
    let mut colors = Vec::with_capacity(positions.capacity());
    let mut indices: Vec<u32> = Vec::with_capacity(trees.len() * (CROWN_SIDES * 2 + TRUNK_SIDES * 2) * 3);

    for t in trees {
        let base = Vec3::new(t.x, t.ground, t.z);
        let h = t.height.max(3.0);
        let r = t.crown_radius.clamp(1.0, h * 0.6);
        let jitter = hash01(t.x, t.z);
        // Darker, bluer greens for taller trees; a little random variation per tree.
        let crown_color = LinearRgba::from(Color::srgb(
            0.20 + 0.10 * jitter - 0.004 * h,
            0.42 + 0.12 * jitter - 0.005 * h,
            0.16 + 0.05 * jitter,
        ))
        .to_f32_array();
        let trunk_color = LinearRgba::from(Color::srgb(0.36, 0.27, 0.18)).to_f32_array();

        // Trunk: triangular prism from the ground to the crown's widest point.
        let trunk_r = (0.08 * h).clamp(0.15, 0.6);
        let crown_bottom = h * 0.25;
        let crown_mid = h * 0.55;
        let start = positions.len() as u32;
        for k in 0..TRUNK_SIDES {
            let a = (k as f32 + 0.5 * jitter) / TRUNK_SIDES as f32 * std::f32::consts::TAU;
            let d = Vec3::new(a.cos(), 0.0, a.sin());
            for y in [0.0, crown_mid] {
                positions.push((base + d * trunk_r + Vec3::Y * y).to_array());
                normals.push(d.to_array());
                colors.push(trunk_color);
            }
        }
        for k in 0..TRUNK_SIDES as u32 {
            let (a0, a1) = (start + 2 * k, start + 2 * k + 1);
            let n = (k + 1) % TRUNK_SIDES as u32;
            let (b0, b1) = (start + 2 * n, start + 2 * n + 1);
            indices.extend_from_slice(&[a0, a1, b0, b0, a1, b1]);
        }

        // Crown: hexagonal bipyramid (ring at crown_mid, tips at crown_bottom and h).
        let start = positions.len() as u32;
        for k in 0..CROWN_SIDES {
            let a = (k as f32 + jitter) / CROWN_SIDES as f32 * std::f32::consts::TAU;
            let d = Vec3::new(a.cos(), 0.0, a.sin());
            positions.push((base + d * r + Vec3::Y * crown_mid).to_array());
            normals.push(d.to_array());
            colors.push(crown_color);
        }
        let top = start + CROWN_SIDES as u32;
        positions.push((base + Vec3::Y * h).to_array());
        normals.push([0.0, 1.0, 0.0]);
        colors.push(crown_color);
        let bottom = top + 1;
        positions.push((base + Vec3::Y * crown_bottom).to_array());
        normals.push([0.0, -1.0, 0.0]);
        colors.push(crown_color);
        for k in 0..CROWN_SIDES as u32 {
            let (a, b) = (start + k, start + (k + 1) % CROWN_SIDES as u32);
            indices.extend_from_slice(&[a, top, b, a, b, bottom]);
        }
    }

    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, colors)
        .with_inserted_indices(Indices::U32(indices))
}
