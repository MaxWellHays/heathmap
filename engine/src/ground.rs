//! Ground material: the terrain's colour comes from signed distance fields (one per
//! ground class, exported by data/export_engine.py), thresholded per pixel in the
//! shader, so area edges stay sharp at any zoom. Lighting and shadows come from
//! Bevy's standard PBR pipeline (this extends `StandardMaterial`).
//!
//! The full-resolution heightmap is also uploaded as a texture: the shader takes exact
//! heights and per-pixel normals from it, so lighting, contour lines and elevation
//! colours always agree with each other (and with the walkable surface), whichever
//! terrain level of detail is drawn.
//!
//! Elevation colours can tint roads and paths too (see `lines.rs`), and their range can
//! follow the heights visible on screen.

use bevy::asset::RenderAssetUsages;
use bevy::image::ImageAddressMode;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat};
use bevy::shader::ShaderRef;

use crate::level::{BinaryFile, Heightmap, Level, LevelHandles, LevelState};
use crate::camera::{CameraMode, Player};
use crate::pick::terrain_hit;
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

/// Same low-blue-to-high-red gradient as the v0 web map (sRGB).
const GRADIENT: [[f32; 3]; 5] = [
    [0.173, 0.482, 0.714],
    [0.671, 0.851, 0.914],
    [0.996, 0.878, 0.565],
    [0.988, 0.553, 0.349],
    [0.843, 0.098, 0.110],
];
/// Fixed elevation range for the gradient (m): the Heath's lowest to highest contour, as in v0.
pub const FIXED_RANGE: (f32, f32) = (24.0, 136.0);
/// The visible range is never narrower than this, so flat views don't exaggerate noise.
const MIN_VISIBLE_SPAN: f32 = 6.0;

fn linear(c: [f32; 3]) -> Vec4 {
    LinearRgba::from(Color::srgb(c[0], c[1], c[2])).to_vec4()
}

/// Elevation colour and contour parameters shared by the ground and line shaders.
#[derive(Clone, Copy, ShaderType, Debug)]
pub struct ElevationUniform {
    /// x: elevation colours on (1/0), y: contours on, z: contour interval (m), w: every n-th line is major.
    pub settings: Vec4,
    /// x, y: elevation range mapped onto the gradient (m), z: colour strength, w: contour strength.
    pub range: Vec4,
    /// Elevation gradient, low to high (linear RGBA), evenly spaced.
    pub gradient: [Vec4; 5],
}

impl Default for ElevationUniform {
    fn default() -> Self {
        ElevationSettings::default().uniform((FIXED_RANGE.0, FIXED_RANGE.1))
    }
}

#[derive(Clone, Copy, ShaderType, Debug)]
pub struct GroundParams {
    /// Linear RGBA: [0] = base, then one per class in paint order.
    pub colors: [Vec4; 13],
    /// Texel value steps per metre in the distance textures (x); rest unused.
    pub sdf_scale: Vec4,
    /// Heightmap texture placement: x, y = world x/z of sample (0, 0), z = metres per sample.
    pub height_grid: Vec4,
    /// Heightmap texture size in samples (x, y).
    pub height_size: Vec4,
    pub elevation: ElevationUniform,
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
    /// Full-resolution heights (R32Float, read with textureLoad, so no filtering needed).
    #[texture(105, sample_type = "float", filterable = false)]
    pub height: Handle<Image>,
}

impl MaterialExtension for GroundExtension {
    fn fragment_shader() -> ShaderRef {
        "shaders/ground.wgsl".into()
    }
}

/// Elevation colour and contour settings shown in the panel (also settable over BRP).
#[derive(Resource, Clone, PartialEq, Reflect)]
#[reflect(Resource)]
pub struct ElevationSettings {
    pub colors: bool,
    pub contours: bool,
    pub contour_interval: f32,
    pub major_every: f32,
    pub color_strength: f32,
    pub contour_strength: f32,
    /// Map the gradient onto the heights visible on screen instead of the fixed range.
    pub auto_range: bool,
}

impl Default for ElevationSettings {
    fn default() -> Self {
        Self {
            colors: false,
            contours: false,
            contour_interval: 2.0,
            major_every: 5.0,
            color_strength: 0.6,
            contour_strength: 0.8,
            auto_range: true,
        }
    }
}

impl ElevationSettings {
    pub fn uniform(&self, (lo, hi): (f32, f32)) -> ElevationUniform {
        ElevationUniform {
            settings: Vec4::new(self.colors as u8 as f32, self.contours as u8 as f32, self.contour_interval, self.major_every),
            range: Vec4::new(lo, hi, self.color_strength, self.contour_strength),
            gradient: GRADIENT.map(linear),
        }
    }
}

/// Heights currently visible on screen (smoothed), for the automatic gradient range.
#[derive(Resource, Clone, Copy)]
pub struct VisibleElevation {
    pub min: f32,
    pub max: f32,
}

impl Default for VisibleElevation {
    fn default() -> Self {
        Self { min: FIXED_RANGE.0, max: FIXED_RANGE.1 }
    }
}

/// The full-resolution heightmap as a texture, shared by the terrain and line shaders.
#[derive(Resource, Clone)]
pub struct HeightTexture {
    pub image: Handle<Image>,
    /// x, y = world x/z of sample (0, 0), z = metres per sample.
    pub grid: Vec4,
    /// Size in samples (x, y).
    pub size: Vec4,
}

pub fn create_height_texture(mut commands: Commands, level: Res<Level>, mut images: ResMut<Assets<Image>>) {
    let hm = &level.heightmap;
    commands.insert_resource(HeightTexture {
        image: images.add(height_texture(hm)),
        grid: Vec4::new(hm.origin.x, hm.origin.y, hm.resolution, 0.0),
        size: Vec4::new(hm.width as f32, hm.height as f32, 0.0, 0.0),
    });
}

/// The terrain's material, so settings changes can update it.
#[derive(Resource)]
pub struct GroundMaterialHandle(pub Handle<GroundMaterial>);

/// Every material that shows elevation colours, with a function to push the uniform into it.
#[derive(Resource, Default)]
pub struct ElevationTargets {
    pub lines: Vec<Handle<crate::lines::LinesMaterial>>,
}

pub struct GroundPlugin;

impl Plugin for GroundPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<GroundMaterial>::default())
            .register_type::<ElevationSettings>()
            .init_resource::<ElevationSettings>()
            .init_resource::<VisibleElevation>()
            .init_resource::<ElevationTargets>()
            .add_systems(OnEnter(LevelState::Ready), create_height_texture)
            .add_systems(Update, (track_visible_elevation, apply_elevation_settings).chain().run_if(in_state(LevelState::Ready)));
    }
}

/// The height "you are at": the player's feet when walking, the ground under the camera
/// when flying, the ground at the centre of the screen in the map view.
fn reference_height(
    mode: CameraMode,
    hm: &Heightmap,
    camera: &Camera,
    cam_tf: &GlobalTransform,
    player: Option<Vec3>,
) -> Option<f32> {
    match mode {
        CameraMode::Walk | CameraMode::ThirdPerson => player.map(|p| p.y),
        CameraMode::Fly => {
            let p = cam_tf.translation();
            Some(hm.sample(p.x, p.z))
        }
        CameraMode::Map => {
            let centre = camera.logical_viewport_size()? / 2.0;
            let ray = camera.viewport_to_world(cam_tf, centre).ok()?;
            terrain_hit(ray, hm, 30_000.0).map(|t| ray.get_point(t).y)
        }
    }
}

/// Samples terrain heights under a grid of screen points a few times a second and eases
/// the visible range towards them, centred on the height you are at: your own level is
/// always the middle colour, and the range reaches as far up and down as the view does.
#[allow(clippy::too_many_arguments)]
fn track_visible_elevation(
    time: Res<Time>,
    settings: Res<ElevationSettings>,
    hm: Res<Heightmap>,
    mode: Res<State<CameraMode>>,
    cams: Query<(&Camera, &GlobalTransform)>,
    players: Query<&Transform, With<Player>>,
    mut visible: ResMut<VisibleElevation>,
    mut timer: Local<f32>,
    mut target: Local<Option<(f32, f32)>>,
) {
    if !settings.auto_range || !(settings.colors || settings.contours) {
        return;
    }
    *timer -= time.delta_secs();
    if *timer <= 0.0 {
        *timer = 0.25;
        if let (Ok((camera, cam_tf)), Some(size)) = (cams.single(), cams.single().ok().and_then(|(c, _)| c.logical_viewport_size())) {
            let mut heights = Vec::new();
            for j in 0..8 {
                for i in 0..12 {
                    let p = Vec2::new((i as f32 + 0.5) / 12.0 * size.x, (j as f32 + 0.5) / 8.0 * size.y);
                    if let Ok(ray) = camera.viewport_to_world(cam_tf, p)
                        && let Some(t) = terrain_hit(ray, &hm, 15_000.0)
                    {
                        heights.push(ray.get_point(t).y);
                    }
                }
            }
            if heights.len() >= 4 {
                heights.sort_by(f32::total_cmp);
                let pick = |q: f32| heights[((heights.len() - 1) as f32 * q).round() as usize];
                let (lo, hi) = (pick(0.02), pick(0.98));
                let centre = reference_height(*mode.get(), &hm, camera, cam_tf, players.single().ok().map(|t| t.translation))
                    .unwrap_or((lo + hi) / 2.0);
                let reach = (centre - lo).max(hi - centre).max(MIN_VISIBLE_SPAN / 2.0);
                *target = Some((centre - reach, centre + reach));
            }
        }
    }
    if let Some((lo, hi)) = *target {
        let k = 1.0 - (-4.0 * time.delta_secs()).exp();
        let (min, max) = (visible.min + (lo - visible.min) * k, visible.max + (hi - visible.max) * k);
        // Only touch the resource on visible changes, so materials aren't rebuilt every frame.
        if (min - visible.min).abs() > 0.05 || (max - visible.max).abs() > 0.05 {
            *visible = VisibleElevation { min, max };
        }
    }
}

/// Pushes elevation settings (and the visible range) into the terrain and line materials.
fn apply_elevation_settings(
    settings: Res<ElevationSettings>,
    visible: Res<VisibleElevation>,
    ground: Option<Res<GroundMaterialHandle>>,
    targets: Res<ElevationTargets>,
    mut ground_materials: ResMut<Assets<GroundMaterial>>,
    mut line_materials: ResMut<Assets<crate::lines::LinesMaterial>>,
) {
    let Some(ground) = ground else { return };
    let auto_changed = settings.auto_range && visible.is_changed();
    if !settings.is_changed() && !auto_changed && !ground.is_added() && !targets.is_changed() {
        return;
    }
    let range = if settings.auto_range { (visible.min, visible.max) } else { FIXED_RANGE };
    let uniform = settings.uniform(range);
    if let Some(mut m) = ground_materials.get_mut(&ground.0) {
        m.extension.params.elevation = uniform;
    }
    for h in &targets.lines {
        if let Some(mut m) = line_materials.get_mut(h) {
            m.extension.elevation = uniform;
        }
    }
}

/// R32Float texture of the full-resolution heights (row 0 = north).
fn height_texture(hm: &Heightmap) -> Image {
    let data: Vec<u8> = hm.heights.iter().flat_map(|h| h.to_le_bytes()).collect();
    Image::new(
        Extent3d { width: hm.width as u32, height: hm.height as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::R32Float,
        RenderAssetUsages::RENDER_WORLD,
    )
}

/// Builds the ground material from the loaded distance fields and heightmap.
pub fn ground_material(
    level: &Level,
    handles: &LevelHandles,
    files: &Assets<BinaryFile>,
    images: &mut Assets<Image>,
    height: &HeightTexture,
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

    let mut colors = [linear(BASE_COLOR); 13];
    for (i, name) in meta.classes.iter().enumerate().take(12) {
        if let Some((_, c)) = CLASS_COLORS.iter().find(|(n, _)| n == name) {
            colors[i + 1] = linear(*c);
        }
    }
    Some(GroundExtension {
        params: GroundParams {
            colors,
            sdf_scale: Vec4::new(meta.scale, 0.0, 0.0, 0.0),
            height_grid: height.grid,
            height_size: height.size,
            elevation: ElevationUniform::default(),
        },
        sdf0,
        sdf1,
        sdf2,
        height: height.image.clone(),
    })
}
