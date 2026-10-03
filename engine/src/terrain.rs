//! Terrain: the heightmap split into square chunks, each with a few levels of
//! detail. Every frame each chunk shows the LOD matching its distance from the
//! camera. Skirts (strips hanging down from chunk edges) hide the cracks where
//! neighbouring chunks use different LODs.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use crate::level::{Heightmap, Level, LevelState};

/// Samples per chunk side (×4 m = 512 m).
const CHUNK_SAMPLES: usize = 128;
/// Sample step for each LOD (1 = full resolution).
const LOD_STEPS: [usize; 4] = [1, 2, 4, 8];
/// Camera distance (m) beyond which each LOD switches to the next coarser one.
const LOD_DISTANCES: [f32; 3] = [700.0, 1500.0, 3000.0];
const SKIRT_DEPTH: f32 = 8.0;

#[derive(Component)]
pub struct TerrainChunk {
    center: Vec3,
    lods: Vec<Entity>,
}

pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(LevelState::Ready), spawn_terrain)
            .add_systems(Update, select_lod.run_if(in_state(LevelState::Ready)));
    }
}

fn spawn_terrain(
    mut commands: Commands,
    level: Res<Level>,
    assets: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let hm = &level.heightmap;
    let material = materials.add(StandardMaterial {
        base_color_texture: Some(assets.load(format!("{}/{}", level.dir, level.meta.ground.file))),
        perceptual_roughness: 1.0,
        reflectance: 0.1,
        ..default()
    });
    let extent = &level.meta.extent;
    let uv_scale = Vec2::new(1.0 / (extent.x[1] - extent.x[0]), 1.0 / (extent.z[1] - extent.z[0]));
    let uv_origin = Vec2::new(extent.x[0], extent.z[0]);

    let mut count = 0;
    for row0 in (0..hm.height - 1).step_by(CHUNK_SAMPLES) {
        for col0 in (0..hm.width - 1).step_by(CHUNK_SAMPLES) {
            let cols = CHUNK_SAMPLES.min(hm.width - 1 - col0);
            let rows = CHUNK_SAMPLES.min(hm.height - 1 - row0);
            let center = hm.world_pos(col0 + cols / 2, row0 + rows / 2);
            let lods = LOD_STEPS
                .iter()
                .enumerate()
                .map(|(i, &step)| {
                    let mesh = chunk_mesh(hm, col0, row0, cols, rows, step, uv_origin, uv_scale);
                    commands
                        .spawn((
                            Mesh3d(meshes.add(mesh)),
                            MeshMaterial3d(material.clone()),
                            if i == 0 { Visibility::Inherited } else { Visibility::Hidden },
                        ))
                        .id()
                })
                .collect::<Vec<_>>();
            commands
                .spawn((TerrainChunk { center, lods: lods.clone() }, Transform::default(), Visibility::default()))
                .add_children(&lods);
            count += 1;
        }
    }
    info!("Spawned {count} terrain chunks × {} LODs", LOD_STEPS.len());
}

#[allow(clippy::too_many_arguments)]
fn chunk_mesh(
    hm: &Heightmap,
    col0: usize,
    row0: usize,
    cols: usize,
    rows: usize,
    step: usize,
    uv_origin: Vec2,
    uv_scale: Vec2,
) -> Mesh {
    // Sample indices along each axis, always including the chunk's last row/column
    // so neighbouring chunks share edge positions.
    let axis = |start: usize, len: usize| {
        let mut v: Vec<usize> = (0..len).step_by(step).map(|i| start + i).collect();
        v.push(start + len);
        v
    };
    let xs = axis(col0, cols);
    let zs = axis(row0, rows);
    let (nx, nz) = (xs.len(), zs.len());

    let mut positions = Vec::with_capacity(nx * nz + 2 * (nx + nz));
    let mut normals = Vec::with_capacity(positions.capacity());
    let mut uvs = Vec::with_capacity(positions.capacity());
    let uv = |p: Vec3| ((Vec2::new(p.x, p.z) - uv_origin) * uv_scale).to_array();

    for &r in &zs {
        for &c in &xs {
            let p = hm.world_pos(c, r);
            positions.push(p.to_array());
            uvs.push(uv(p));
            normals.push(normal_at(hm, c, r).to_array());
        }
    }
    let mut indices: Vec<u32> = Vec::with_capacity((nx - 1) * (nz - 1) * 6);
    let idx = |i: usize, j: usize| (j * nx + i) as u32;
    for j in 0..nz - 1 {
        for i in 0..nx - 1 {
            let (a, b, c, d) = (idx(i, j), idx(i + 1, j), idx(i, j + 1), idx(i + 1, j + 1));
            // Counter-clockwise when seen from above (+y): north is −z.
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }

    // Skirts: copy each edge vertex straight down and stitch a strip between them.
    let edges: [Vec<u32>; 4] = [
        (0..nx).map(|i| idx(i, 0)).collect(),                 // north
        (0..nz).map(|j| idx(nx - 1, j)).collect(),            // east
        (0..nx).rev().map(|i| idx(i, nz - 1)).collect(),      // south
        (0..nz).rev().map(|j| idx(0, j)).collect(),           // west
    ];
    for edge in edges {
        let base = positions.len() as u32;
        for &v in &edge {
            let mut p = positions[v as usize];
            p[1] -= SKIRT_DEPTH;
            positions.push(p);
            normals.push(normals[v as usize]);
            uvs.push(uvs[v as usize]);
        }
        for k in 0..edge.len() as u32 - 1 {
            let (top_a, top_b) = (edge[k as usize], edge[k as usize + 1]);
            let (bot_a, bot_b) = (base + k, base + k + 1);
            indices.extend_from_slice(&[top_a, top_b, bot_a, top_b, bot_b, bot_a]);
        }
    }

    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_indices(Indices::U32(indices))
}

/// Normal from central differences on the full-resolution grid (also used by coarse LODs,
/// so lighting stays detailed even where geometry is simplified).
fn normal_at(hm: &Heightmap, c: usize, r: usize) -> Vec3 {
    let l = hm.at_index(c.saturating_sub(1), r);
    let rr = hm.at_index(c + 1, r);
    let u = hm.at_index(c, r.saturating_sub(1));
    let d = hm.at_index(c, r + 1);
    Vec3::new(l - rr, 2.0 * hm.resolution, u - d).normalize()
}

fn select_lod(
    camera: Query<&GlobalTransform, With<Camera3d>>,
    chunks: Query<&TerrainChunk>,
    mut visibility: Query<&mut Visibility>,
) {
    let Ok(cam) = camera.single() else { return };
    let eye = cam.translation();
    for chunk in &chunks {
        let d = eye.distance(chunk.center);
        let lod = LOD_DISTANCES.iter().take_while(|&&limit| d > limit).count();
        for (i, &e) in chunk.lods.iter().enumerate() {
            if let Ok(mut v) = visibility.get_mut(e) {
                let want = if i == lod { Visibility::Inherited } else { Visibility::Hidden };
                v.set_if_neq(want);
            }
        }
    }
}
