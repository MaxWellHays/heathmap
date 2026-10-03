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
    ("water", [0.16, 0.30, 0.36]), // same as the water surface, so their edges blend
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
    /// x: elevation colours on (1/0), y: contours on, z: contour interval (m), w: every n-th line is major.
    pub elevation: Vec4,
    /// x, y: elevation range mapped onto the gradient (m), z: colour strength, w: contour strength.
    pub elevation_range: Vec4,
    /// Elevation gradient, low to high (linear RGBA), evenly spaced.
    pub gradient: [Vec4; 5],
}

/// Same low-blue-to-high-red gradient as the v0 web map (sRGB).
const GRADIENT: [[f32; 3]; 5] = [
    [0.173, 0.482, 0.714],
    [0.671, 0.851, 0.914],
    [0.996, 0.878, 0.565],
    [0.988, 0.553, 0.349],
    [0.843, 0.098, 0.110],
];
/// Elevation range for the gradient (m): the Heath's lowest to highest contour, as in v0.
pub const ELEVATION_RANGE: (f32, f32) = (24.0, 136.0);

/// Elevation colour and contour settings shown in the panel.
#[derive(Resource, Clone, PartialEq)]
pub struct ElevationSettings {
    pub colors: bool,
    pub contours: bool,
    pub contour_interval: f32,
    pub major_every: f32,
    pub color_strength: f32,
    pub contour_strength: f32,
}

impl Default for ElevationSettings {
    fn default() -> Self {
        Self { colors: false, contours: false, contour_interval: 2.0, major_every: 5.0, color_strength: 0.55, contour_strength: 0.8 }
    }
}

impl ElevationSettings {
    fn uniforms(&self) -> (Vec4, Vec4) {
        (
            Vec4::new(self.colors as u8 as f32, self.contours as u8 as f32, self.contour_interval, self.major_every),
            Vec4::new(ELEVATION_RANGE.0, ELEVATION_RANGE.1, self.color_strength, self.contour_strength),
        )
    }
}

/// The terrain's material, so settings changes can update it.
#[derive(Resource)]
pub struct GroundMaterialHandle(pub Handle<GroundMaterial>);

/// Pushes panel changes to elevation colours / contours into the terrain material.
pub fn apply_elevation_settings(
    settings: Res<ElevationSettings>,
    handle: Option<Res<GroundMaterialHandle>>,
    mut materials: ResMut<Assets<GroundMaterial>>,
) {
    let Some(handle) = handle else { return };
    if !settings.is_changed() && !handle.is_added() {
        return;
    }
    if let Some(mut m) = materials.get_mut(&handle.0) {
        let (elevation, range) = settings.uniforms();
        m.extension.params.elevation = elevation;
        m.extension.params.elevation_range = range;
    }
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
    let (elevation, elevation_range) = ElevationSettings::default().uniforms();
    let gradient = GRADIENT.map(linear);
    Some(GroundExtension {
        params: GroundParams { colors, sdf_scale: Vec4::new(meta.scale, 0.0, 0.0, 0.0), elevation, elevation_range, gradient },
        sdf0,
        sdf1,
        sdf2,
    })
}
