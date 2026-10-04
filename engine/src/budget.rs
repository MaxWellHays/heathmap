//! Rendering settings that follow the camera's height above the ground, so close-up views
//! stay sharp and overviews of the whole city stay affordable:
//! - shadow cascades: tight and sharp when walking, stretched (up to 4 km) from above;
//! - shadow map resolution: 4096 near the ground, 2048 from high up;
//! - depth prepass near the ground only: there overlapping tree crowns make shading the
//!   bottleneck (the prepass cut the main pass ~6×), while from above the extra geometry
//!   pass costs more than it saves.
//!
//! WebGL2 supports a single shadow cascade (Bevy drops the rest), so there one cascade
//! covers the whole shadow range.

use bevy::core_pipeline::prepass::DepthPrepass;
use bevy::light::{CascadeShadowConfig, CascadeShadowConfigBuilder, DirectionalLightShadowMap};
use bevy::prelude::*;

use crate::level::Heightmap;
use crate::ui::Sun;

const WEBGL2: bool = cfg!(all(target_arch = "wasm32", not(feature = "webgpu")));
const CASCADES: usize = if WEBGL2 { 1 } else { 4 };
const MAX_SHADOW_DISTANCE: f32 = 4000.0; // m
const SHARP_SHADOWS_BELOW: f32 = 500.0; // m above ground: 4096 shadow maps below, 2048 above
const PREPASS_ON_BELOW: f32 = 250.0; // m above ground
const PREPASS_OFF_ABOVE: f32 = 350.0;

pub struct BudgetPlugin;

impl Plugin for BudgetPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(DirectionalLightShadowMap { size: 4096 }).add_systems(Update, follow_camera_height);
    }
}

/// Shadow cascades covering `max_distance` from the camera.
pub fn shadow_cascades(max_distance: f32) -> CascadeShadowConfig {
    CascadeShadowConfigBuilder {
        num_cascades: CASCADES,
        first_cascade_far_bound: if CASCADES == 1 { max_distance } else { (max_distance / 12.0).max(15.0) },
        maximum_distance: max_distance,
        ..default()
    }
    .build()
}

fn follow_camera_height(
    mut commands: Commands,
    camera: Query<(Entity, &Transform, Has<DepthPrepass>), With<Camera3d>>,
    heightmap: Option<Res<Heightmap>>,
    mut sun: Query<&mut CascadeShadowConfig, With<Sun>>,
    mut shadow_map: ResMut<DirectionalLightShadowMap>,
    mut current: Local<f32>,
) {
    let (Ok((entity, cam, prepass)), Some(hm), Ok(mut cascades)) = (camera.single(), heightmap, sun.single_mut()) else { return };
    let altitude = (cam.translation.y - hm.sample(cam.translation.x, cam.translation.z)).max(1.7);

    let wanted = (altitude * 4.0).clamp(150.0, MAX_SHADOW_DISTANCE);
    // Rebuild only on meaningful changes (cascade splits are recomputed from scratch).
    if (wanted / current.max(1.0) - 1.0).abs() > 0.15 {
        *current = wanted;
        *cascades = shadow_cascades(wanted);
    }

    // A single WebGL2 cascade spans the whole range, so it keeps the full resolution.
    let size = if WEBGL2 || altitude < SHARP_SHADOWS_BELOW { 4096 } else { 2048 };
    if shadow_map.size != size {
        shadow_map.size = size;
    }

    if !prepass && altitude < PREPASS_ON_BELOW {
        commands.entity(entity).insert(DepthPrepass);
    } else if prepass && altitude > PREPASS_OFF_ABOVE {
        commands.entity(entity).remove::<DepthPrepass>();
    }
}
