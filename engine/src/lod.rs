//! Distance-based level of detail shared by terrain, trees and anything else that
//! is split into chunks: each chunk has one child entity per LOD, and every frame
//! the one matching the camera distance is shown.

use bevy::prelude::*;

#[derive(Component)]
pub struct LodChunk {
    pub center: Vec3,
    /// One entity per level, finest first. `Entity::PLACEHOLDER` means "draw nothing".
    pub levels: Vec<Entity>,
    /// Distance at which each level hands over to the next (len = levels.len() - 1).
    pub distances: &'static [f32],
}

pub struct LodPlugin;

impl Plugin for LodPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, select_lod.before(TransformSystems::Propagate));
    }
}

fn select_lod(
    camera: Query<&Transform, With<Camera3d>>,
    chunks: Query<&LodChunk>,
    mut visibility: Query<&mut Visibility>,
) {
    let Ok(cam) = camera.single() else { return };
    let eye = cam.translation;
    for chunk in &chunks {
        let d = eye.distance(chunk.center);
        let lod = chunk.distances.iter().take_while(|&&limit| d > limit).count();
        for (i, &e) in chunk.levels.iter().enumerate() {
            if let Ok(mut v) = visibility.get_mut(e) {
                v.set_if_neq(if i == lod { Visibility::Inherited } else { Visibility::Hidden });
            }
        }
    }
}
