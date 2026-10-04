//! Level data exported by `data/export_engine.py`, loaded through Bevy's asset
//! system so the same code works natively and in the browser.

use bevy::asset::{AssetLoader, LoadContext, io::Reader};
use bevy::prelude::*;
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
pub struct TerrainMeta {
    pub file: String,
    pub width: usize,
    pub height: usize,
    pub resolution: f32,
    pub min_ele: f32,
    #[allow(dead_code)]
    pub max_ele: f32,
}

#[derive(Deserialize, Debug, Clone)]
pub struct FileMeta {
    pub file: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct SdfMeta {
    pub file: String,
    pub width: usize,
    pub height: usize,
    pub scale: f32,
    pub classes: Vec<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Extent {
    pub x: [f32; 2],
    pub z: [f32; 2],
}

/// `level.json`
#[derive(Asset, TypePath, Deserialize, Debug, Clone)]
pub struct LevelMeta {
    pub name: String,
    pub extent: Extent,
    pub terrain: TerrainMeta,
    pub ground_sdf: SdfMeta,
    pub trees: FileMeta,
    pub buildings: FileMeta,
    pub lines: FileMeta,
    pub water: FileMeta,
    pub props: FileMeta,
    pub barriers: FileMeta,
    pub landmarks: FileMeta,
    /// How GPS positions map into the level (for imported runs); missing in older exports.
    #[serde(default)]
    pub georef: Option<Georef>,
}

/// WGS84 → level coordinates: a cubic polynomial in (lon − lon0, lat − lat0) × scale,
/// fitted to the exact projection by `data/export_engine.py`.
#[derive(Deserialize, Debug, Clone)]
pub struct Georef {
    pub lon0: f64,
    pub lat0: f64,
    pub scale: f64,
    /// Powers (i, j) of u and v for each coefficient.
    pub terms: Vec<[i32; 2]>,
    pub x: Vec<f64>,
    pub z: Vec<f64>,
}

impl Georef {
    pub fn to_level(&self, lat: f64, lon: f64) -> Vec2 {
        let (u, v) = ((lon - self.lon0) * self.scale, (lat - self.lat0) * self.scale);
        let (mut x, mut z) = (0.0, 0.0);
        for (k, [i, j]) in self.terms.iter().enumerate() {
            let m = u.powi(*i) * v.powi(*j);
            x += self.x[k] * m;
            z += self.z[k] * m;
        }
        Vec2::new(x as f32, z as f32)
    }
}

/// Raw bytes of a `.bin` file; decoded once all level files have arrived. The files may be
/// gzipped (the export compresses them, so they download quickly from a static web host).
#[derive(Asset, TypePath, Debug)]
pub struct BinaryFile(pub Vec<u8>);

#[derive(Default, TypePath)]
struct LevelMetaLoader;

impl AssetLoader for LevelMetaLoader {
    type Asset = LevelMeta;
    type Settings = ();
    type Error = BevyError;

    async fn load(&self, reader: &mut dyn Reader, _: &(), _: &mut LoadContext<'_>) -> Result<LevelMeta, BevyError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn extensions(&self) -> &[&str] {
        &["level.json"]
    }
}

#[derive(Default, TypePath)]
struct BinaryFileLoader;

impl AssetLoader for BinaryFileLoader {
    type Asset = BinaryFile;
    type Settings = ();
    type Error = BevyError;

    async fn load(&self, reader: &mut dyn Reader, _: &(), _: &mut LoadContext<'_>) -> Result<BinaryFile, BevyError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        if bytes.starts_with(&[0x1f, 0x8b]) {
            let mut out = Vec::new();
            std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&bytes[..]), &mut out)?;
            bytes = out;
        }
        Ok(BinaryFile(bytes))
    }

    fn extensions(&self) -> &[&str] {
        &["bin"]
    }
}

/// Terrain heights on a regular grid, row 0 at the north edge (−z), column 0 at the west edge (−x).
#[derive(Resource, Clone)]
pub struct Heightmap {
    pub width: usize,
    pub height: usize,
    pub resolution: f32,
    /// World position of sample (0, 0).
    pub origin: Vec2,
    pub heights: Vec<f32>,
}

impl Heightmap {
    pub fn from_bytes(meta: &TerrainMeta, extent: &Extent, bytes: &[u8]) -> Self {
        let heights = bytes
            .chunks_exact(2)
            .map(|b| meta.min_ele + u16::from_le_bytes([b[0], b[1]]) as f32 / 100.0)
            .collect();
        let half = meta.resolution / 2.0;
        Self {
            width: meta.width,
            height: meta.height,
            resolution: meta.resolution,
            origin: Vec2::new(extent.x[0] + half, extent.z[0] + half),
            heights,
        }
    }

    pub fn at_index(&self, col: usize, row: usize) -> f32 {
        self.heights[row.min(self.height - 1) * self.width + col.min(self.width - 1)]
    }

    pub fn world_pos(&self, col: usize, row: usize) -> Vec3 {
        let p = self.origin + Vec2::new(col as f32, row as f32) * self.resolution;
        Vec3::new(p.x, self.at_index(col, row), p.y)
    }

    /// Like `sample`, but on the coarser grid used by a terrain LOD with this sample step
    /// (chunk origins are multiples of every step, so the grids line up with the meshes).
    pub fn sample_step(&self, x: f32, z: f32, step: usize) -> f32 {
        if step <= 1 {
            return self.sample(x, z);
        }
        let s = step as f32;
        let fx = ((x - self.origin.x) / self.resolution / s).max(0.0);
        let fz = ((z - self.origin.y) / self.resolution / s).max(0.0);
        let (c, r) = (fx.floor(), fz.floor());
        let (tx, tz) = (fx - c, fz - r);
        let at = |cc: f32, rr: f32| self.at_index((cc * s) as usize, (rr * s) as usize);
        let (a, b, cc) = (at(c, r), at(c + 1.0, r), at(c, r + 1.0));
        if tx + tz <= 1.0 {
            a + (b - a) * tx + (cc - a) * tz
        } else {
            let d = at(c + 1.0, r + 1.0);
            d + (cc - d) * (1.0 - tx) + (b - d) * (1.0 - tz)
        }
    }

    /// Terrain height at a world (x, z) position, interpolated on the same triangles as
    /// the full-detail terrain mesh (each cell split along its NE–SW diagonal), so things
    /// placed with it sit exactly on the visible surface.
    pub fn sample(&self, x: f32, z: f32) -> f32 {
        let fx = ((x - self.origin.x) / self.resolution).clamp(0.0, (self.width - 1) as f32);
        let fz = ((z - self.origin.y) / self.resolution).clamp(0.0, (self.height - 1) as f32);
        let (c, r) = (fx.floor() as usize, fz.floor() as usize);
        let (tx, tz) = (fx - c as f32, fz - r as f32);
        let a = self.at_index(c, r); // north-west
        let b = self.at_index(c + 1, r); // north-east
        let cc = self.at_index(c, r + 1); // south-west
        if tx + tz <= 1.0 {
            a + (b - a) * tx + (cc - a) * tz
        } else {
            let d = self.at_index(c + 1, r + 1); // south-east
            d + (cc - d) * (1.0 - tx) + (b - d) * (1.0 - tz)
        }
    }
}

/// One tree from `trees.bin`.
#[derive(Clone, Copy)]
pub struct Tree {
    pub x: f32,
    pub z: f32,
    #[allow(dead_code)] // trees are placed on the rendered terrain instead
    pub ground: f32,
    pub height: f32,
    pub crown_radius: f32,
}

pub fn decode_trees(bytes: &[u8]) -> Vec<Tree> {
    bytes
        .chunks_exact(20)
        .map(|r| {
            let f = |i: usize| f32::from_le_bytes([r[i * 4], r[i * 4 + 1], r[i * 4 + 2], r[i * 4 + 3]]);
            Tree { x: f(0), z: f(1), ground: f(2), height: f(3), crown_radius: f(4) }
        })
        .collect()
}

/// Frames between spawning heavy layers (terrain, trees, buildings, roads, props).
const FRAMES_PER_STEP: u32 = 3;
/// Spawn steps, in order: terrain, trees, buildings, roads, props.
const SPAWN_STEPS: u32 = 5;

/// The heavy layers spawn one after another, a few frames apart, each once its file has
/// arrived: so each one's meshes are uploaded to the GPU and freed before the next is built
/// (spawning them all in one frame peaked near 4 GB, the most a browser (wasm32) can
/// address), and in the browser the scene appears layer by layer as files download.
#[derive(Resource)]
pub struct SpawnSequence {
    next: u32,
    wait: u32,
}

impl Default for SpawnSequence {
    fn default() -> Self {
        Self { next: 1, wait: 0 }
    }
}

impl SpawnSequence {
    /// The current step has spawned; the next one may run a few frames from now.
    pub fn done(&mut self) {
        self.next += 1;
        self.wait = FRAMES_PER_STEP;
    }

    /// All layers are in.
    pub fn finished(&self) -> bool {
        self.next > SPAWN_STEPS
    }
}

/// Run condition: true while spawn step `step` is due (until its system calls `done`;
/// a system whose file hasn't arrived yet just returns and runs again next frame).
pub fn spawn_step(step: u32) -> impl Fn(Option<Res<SpawnSequence>>, Option<Res<State<LevelState>>>) -> bool + Clone {
    move |seq, state| {
        state.is_some_and(|s| *s.get() == LevelState::Ready) && seq.is_some_and(|q| q.next == step && q.wait == 0)
    }
}

fn count_spawn_frames(mut seq: ResMut<SpawnSequence>) {
    seq.wait = seq.wait.saturating_sub(1);
}

#[derive(States, Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LevelState {
    #[default]
    LoadingMeta,
    LoadingData,
    Ready,
}

#[derive(Resource)]
pub struct LevelHandles {
    pub dir: String,
    pub meta: Handle<LevelMeta>,
    pub terrain: Option<Handle<BinaryFile>>,
    pub trees: Option<Handle<BinaryFile>>,
    pub buildings: Option<Handle<BinaryFile>>,
    pub lines: Option<Handle<BinaryFile>>,
    pub ground_sdf: Option<Handle<BinaryFile>>,
    pub water: Option<Handle<BinaryFile>>,
    pub props: Option<Handle<BinaryFile>>,
    pub barriers: Option<Handle<BinaryFile>>,
    pub landmarks: Option<Handle<BinaryFile>>,
}

/// The loaded level, available once `LevelState::Ready` is reached.
#[derive(Resource)]
pub struct Level {
    pub meta: LevelMeta,
    pub heightmap: Heightmap,
    pub trees: Vec<Tree>,
}

pub struct LevelPlugin {
    pub dir: &'static str,
}

impl Plugin for LevelPlugin {
    fn build(&self, app: &mut App) {
        let dir = self.dir.to_string();
        app.init_asset::<LevelMeta>()
            .init_asset::<BinaryFile>()
            .register_asset_loader(LevelMetaLoader)
            .register_asset_loader(BinaryFileLoader)
            .init_state::<LevelState>()
            .init_resource::<SpawnSequence>()
            .add_systems(Last, count_spawn_frames.run_if(in_state(LevelState::Ready)))
            .add_systems(Startup, move |mut commands: Commands, assets: Res<AssetServer>| {
                commands.insert_resource(LevelHandles {
                    meta: assets.load(format!("{dir}/level.json")),
                    dir: dir.clone(),
                    terrain: None,
                    trees: None,
                    buildings: None,
                    lines: None,
                    ground_sdf: None,
                    water: None,
                    props: None,
                    barriers: None,
                    landmarks: None,
                });
            })
            .add_systems(Update, request_data.run_if(in_state(LevelState::LoadingMeta)))
            .add_systems(Update, finish_loading.run_if(in_state(LevelState::LoadingData)))
            .add_systems(OnEnter(LevelState::Ready), request_rest);
    }
}

fn request_data(
    mut handles: ResMut<LevelHandles>,
    metas: Res<Assets<LevelMeta>>,
    assets: Res<AssetServer>,
    mut next: ResMut<NextState<LevelState>>,
) {
    let Some(meta) = metas.get(&handles.meta) else { return };
    // What the first picture needs; buildings, roads and props follow (`request_rest`), so
    // on a slow connection the scene appears before everything has downloaded.
    let dir = handles.dir.clone();
    handles.terrain = Some(assets.load(format!("{dir}/{}", meta.terrain.file)));
    handles.ground_sdf = Some(assets.load(format!("{dir}/{}", meta.ground_sdf.file)));
    handles.trees = Some(assets.load(format!("{dir}/{}", meta.trees.file)));
    handles.water = Some(assets.load(format!("{dir}/{}", meta.water.file)));
    handles.landmarks = Some(assets.load(format!("{dir}/{}", meta.landmarks.file)));
    next.set(LevelState::LoadingData);
}

fn request_rest(mut handles: ResMut<LevelHandles>, metas: Res<Assets<LevelMeta>>, assets: Res<AssetServer>) {
    let Some(meta) = metas.get(&handles.meta).cloned() else { return };
    let dir = handles.dir.clone();
    handles.buildings = Some(assets.load(format!("{dir}/{}", meta.buildings.file)));
    handles.lines = Some(assets.load(format!("{dir}/{}", meta.lines.file)));
    handles.props = Some(assets.load(format!("{dir}/{}", meta.props.file)));
    handles.barriers = Some(assets.load(format!("{dir}/{}", meta.barriers.file)));
}

fn finish_loading(
    mut commands: Commands,
    handles: Res<LevelHandles>,
    metas: Res<Assets<LevelMeta>>,
    files: Res<Assets<BinaryFile>>,
    mut next: ResMut<NextState<LevelState>>,
) {
    let (Some(meta), Some(terrain), Some(trees)) = (
        metas.get(&handles.meta),
        handles.terrain.as_ref().and_then(|h| files.get(h)),
        handles.trees.as_ref().and_then(|h| files.get(h)),
    ) else {
        return;
    };
    // The others are decoded by their own plugins; just wait for them to arrive.
    let arrived = |h: &Option<Handle<BinaryFile>>| h.as_ref().is_some_and(|h| files.contains(h));
    let files_needed = [&handles.ground_sdf, &handles.water, &handles.landmarks];
    if !files_needed.into_iter().all(arrived) {
        return;
    }
    let heightmap = Heightmap::from_bytes(&meta.terrain, &meta.extent, &terrain.0);
    let trees = decode_trees(&trees.0);
    info!("Level '{}' loaded: {}×{} terrain, {} trees", meta.name, heightmap.width, heightmap.height, trees.len());
    commands.insert_resource(heightmap.clone());
    commands.insert_resource(Level { meta: meta.clone(), heightmap, trees });
    next.set(LevelState::Ready);
}
