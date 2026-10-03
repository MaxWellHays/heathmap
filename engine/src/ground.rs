//! Ground material: the terrain's colour comes from signed distance fields (one per
//! ground class, exported by data/export_engine.py), thresholded per pixel in the
//! shader, so area edges stay sharp at any zoom. Lighting and shadows come from
//! Bevy's standard PBR pipeline (this extends `StandardMaterial`).

use bevy::image::ImageAddressMode;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType, TextureFormat};
use bevy::shader::ShaderRef;

use crate::level::{BinaryFile, Level, LevelHandles};
use crate::textures::image_with_mips;

pub type GroundMaterial = ExtendedMaterial<StandardMaterial, GroundExtension>;

/// Must match SDF_CLASSES in data/export_engine.py; colours are sRGB.
const CLASS_COLORS: [(&str, [f32; 3]); 12] = [
    ("park", [0.72, 0.81, 0.58]),
    ("grass", [0.75, 0.84, 0.61]),
    ("meadow", [0.79, 0.85, 0.60]),
    ("scrub", [0.60, 0.72, 0.47]),
    ("heath", [0.73, 0.69, 0.47]),
    ("wood", [0.50, 0.64, 0.39]),
    ("cemetery", [0.66, 0.74, 0.60]),
    ("pitch", [0.66, 0.82, 0.54]),
    ("wetland", [0.61, 0.78, 0.71]),
    ("water", [0.50, 0.70, 0.84]),
    ("road", [0.62, 0.62, 0.63]),
    ("road_major", [0.58, 0.58, 0.60]),
];
const BASE_COLOR: [f32; 3] = [0.86, 0.85, 0.82]; // urban ground between everything

#[derive(Clone, Copy, ShaderType, Debug)]
pub struct GroundParams {
    /// Linear RGBA: [0] = base, then one per class in paint order.
    pub colors: [Vec4; 13],
    /// Texel value steps per metre in the distance textures (x); rest unused.
    pub sdf_scale: Vec4,
}

#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct GroundExtension {
    #[uniform(100)]
    pub params: GroundParams,
    #[texture(101)]
    #[sampler(102)]
    pub sdf0: Handle<Image>,
    #[texture(103)]
    pub sdf1: Handle<Image>,
    #[texture(104)]
    pub sdf2: Handle<Image>,
}

impl MaterialExtension for GroundExtension {
    fn fragment_shader() -> ShaderRef {
        "shaders/ground.wgsl".into()
    }
}

/// Builds the ground material from the loaded distance fields.
pub fn ground_material(
    level: &Level,
    handles: &LevelHandles,
    files: &Assets<BinaryFile>,
    images: &mut Assets<Image>,
) -> Option<GroundExtension> {
    let meta = &level.meta.ground_sdf;
    let bytes = &files.get(handles.ground_sdf.as_ref()?)?.0;
    let layer_len = meta.width * meta.height * 4;
    if bytes.len() < layer_len * 3 {
        warn!("ground_sdf.bin is too short");
        return None;
    }
    let mut layer = |i: usize| {
        let data = bytes[i * layer_len..(i + 1) * layer_len].to_vec();
        images.add(image_with_mips(
            meta.width,
            meta.height,
            data,
            TextureFormat::Rgba8Unorm, // distances, not colours: no sRGB conversion
            ImageAddressMode::ClampToEdge,
            ImageAddressMode::ClampToEdge,
        ))
    };
    let (sdf0, sdf1, sdf2) = (layer(0), layer(1), layer(2));

    let linear = |c: [f32; 3]| LinearRgba::from(Color::srgb(c[0], c[1], c[2])).to_vec4();
    let mut colors = [linear(BASE_COLOR); 13];
    for (i, name) in meta.classes.iter().enumerate().take(12) {
        if let Some((_, c)) = CLASS_COLORS.iter().find(|(n, _)| n == name) {
            colors[i + 1] = linear(*c);
        }
    }
    Some(GroundExtension {
        params: GroundParams { colors, sdf_scale: Vec4::new(meta.scale, 0.0, 0.0, 0.0) },
        sdf0,
        sdf1,
        sdf2,
    })
}
