//! Your runs (exported from Strava by data/export_engine.py): each route drawn as a thin
//! coloured line on the ground, and a ghost runner per run replaying it at the pace you
//! actually ran (all starting together; playback speed adjustable from the panel).

use bevy::prelude::*;

use crate::avatar::{FigureColors, Gait, spawn_figure};
use crate::buildings::Reader;
use crate::level::{BinaryFile, Heightmap, LevelHandles, LevelState};
use crate::props::Shapes;

const ROUTE_WIDTH: f32 = 0.5;
const ROUTE_LIFT: f32 = 0.16; // just above the highest road surface

pub struct Run {
    #[allow(dead_code)] // for matching runs with Strava later
    pub id: u64,
    #[allow(dead_code)]
    pub start: i64,
    /// (position, seconds since start)
    pub points: Vec<(Vec2, f32)>,
}

impl Run {
    pub fn duration(&self) -> f32 {
        self.points.last().map_or(0.0, |p| p.1)
    }

    /// Position, direction and speed (m/s) at `t` seconds into the run.
    fn sample(&self, t: f32) -> (Vec2, Vec2, f32) {
        let k = self.points.partition_point(|p| p.1 <= t).clamp(1, self.points.len() - 1);
        let ((a, ta), (b, tb)) = (self.points[k - 1], self.points[k]);
        let span = (tb - ta).max(1e-3);
        let f = ((t - ta) / span).clamp(0.0, 1.0);
        (a.lerp(b, f), (b - a).normalize_or(Vec2::NEG_Y), a.distance(b) / span)
    }
}

#[derive(Resource, Default)]
pub struct Runs(pub Vec<Run>);

/// Playback state for the ghosts, controlled from the panel.
#[derive(Resource)]
pub struct RunPlayback {
    pub time: f32,
    pub speed: f32,
    pub playing: bool,
    pub show_routes: bool,
    pub show_ghosts: bool,
}

impl Default for RunPlayback {
    fn default() -> Self {
        Self { time: 0.0, speed: 1.0, playing: true, show_routes: true, show_ghosts: true }
    }
}

#[derive(Component)]
struct Ghost(usize);

#[derive(Component)]
pub struct Routes;

#[derive(Component)]
pub struct Ghosts;

pub struct RunsPlugin;

impl Plugin for RunsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Runs>()
            .init_resource::<RunPlayback>()
            .add_systems(OnEnter(LevelState::Ready), spawn_runs)
            .add_systems(Update, (move_ghosts, apply_visibility).run_if(in_state(LevelState::Ready)));
    }
}

fn decode(bytes: &[u8]) -> Vec<Run> {
    let mut r = Reader { bytes, pos: 0 };
    let count = r.u32();
    (0..count)
        .map(|_| {
            let id = u64::from(r.u32()) | (u64::from(r.u32()) << 32);
            let start = (u64::from(r.u32()) | (u64::from(r.u32()) << 32)) as i64;
            let points = (0..r.u32()).map(|_| (Vec2::new(r.f32(), r.f32()), r.f32())).collect();
            Run { id, start, points }
        })
        .collect()
}

/// A distinct, saturated colour per run.
fn run_color(i: usize) -> Color {
    Color::hsl((i as f32 * 137.5) % 360.0, 0.85, 0.5)
}

fn spawn_runs(
    mut commands: Commands,
    handles: Res<LevelHandles>,
    files: Res<Assets<BinaryFile>>,
    hm: Res<Heightmap>,
    mut runs: ResMut<Runs>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(file) = handles.runs.as_ref().and_then(|h| files.get(h)) else { return };
    runs.0 = decode(&file.0).into_iter().filter(|r| r.points.len() >= 2).collect();
    if runs.0.is_empty() {
        return;
    }

    // Routes: one mesh of thin draped strips, unlit so their colours read clearly.
    let mut shapes = Shapes::default();
    for (i, run) in runs.0.iter().enumerate() {
        let color = LinearRgba::from(run_color(i)).to_f32_array();
        for w in run.points.windows(2) {
            let (a, b) = (w[0].0, w[1].0);
            if a.distance(b) < 0.2 || a.distance(b) > 50.0 {
                continue; // duplicate points or GPS gaps
            }
            let side = Vec2::new(-(b - a).y, (b - a).x).normalize() * ROUTE_WIDTH / 2.0;
            // Each route a few millimetres apart in height, so overlapping routes don't flicker.
            let lift = ROUTE_LIFT + i as f32 * 0.004;
            let at = |p: Vec2| Vec3::new(p.x, hm.sample(p.x, p.y) + lift, p.y);
            let (p0, p1, p2, p3) = (at(a - side), at(b - side), at(b + side), at(a + side));
            if (p1 - p0).cross(p3 - p0).y >= 0.0 {
                shapes.quad(p0, p1, p2, p3, color);
            } else {
                shapes.quad(p0, p3, p2, p1, color);
            }
        }
    }
    let route_material = materials.add(StandardMaterial { unlit: true, double_sided: true, cull_mode: None, ..default() });
    commands.spawn((Routes, Name::new("Run routes"), Mesh3d(meshes.add(shapes.build())), MeshMaterial3d(route_material)));

    // Ghosts: one runner per run, shirt in the route's colour.
    let ghosts = commands.spawn((Ghosts, Name::new("Ghost runners"), Transform::default(), Visibility::default())).id();
    for (i, run) in runs.0.iter().enumerate() {
        let p = run.points[0].0;
        let ghost = commands
            .spawn((Ghost(i), Gait::default(), Transform::from_xyz(p.x, hm.sample(p.x, p.y), p.y), Visibility::default()))
            .id();
        let colors = FigureColors { shirt: run_color(i), ..default() };
        spawn_figure(&mut commands, ghost, colors, &mut meshes, &mut materials);
        commands.entity(ghosts).add_child(ghost);
    }
    info!("Loaded {} runs ({} points)", runs.0.len(), runs.0.iter().map(|r| r.points.len()).sum::<usize>());
}

/// Advances playback and places each ghost where you were at that moment of the run
/// (looping each run from its start when it ends).
fn move_ghosts(
    time: Res<Time>,
    runs: Res<Runs>,
    hm: Res<Heightmap>,
    mut playback: ResMut<RunPlayback>,
    mut ghosts: Query<(&Ghost, &mut Transform, &mut Gait)>,
) {
    if playback.playing {
        playback.time += time.delta_secs() * playback.speed;
    }
    for (ghost, mut tf, mut gait) in &mut ghosts {
        let run = &runs.0[ghost.0];
        let t = playback.time % run.duration().max(1.0);
        let (p, dir, speed) = run.sample(t);
        tf.translation = Vec3::new(p.x, hm.sample(p.x, p.y), p.y);
        tf.rotation = Quat::from_rotation_y((-dir.x).atan2(-dir.y));
        // The figure animates at the speed it appears to move.
        gait.speed = if playback.playing { (speed * playback.speed).min(30.0) } else { 0.0 };
    }
}

fn apply_visibility(
    playback: Res<RunPlayback>,
    mut routes: Query<&mut Visibility, (With<Routes>, Without<Ghosts>)>,
    mut ghosts: Query<&mut Visibility, (With<Ghosts>, Without<Routes>)>,
) {
    if !playback.is_changed() {
        return;
    }
    let vis = |on: bool| if on { Visibility::Inherited } else { Visibility::Hidden };
    routes.iter_mut().for_each(|mut v| *v = vis(playback.show_routes));
    ghosts.iter_mut().for_each(|mut v| *v = vis(playback.show_ghosts));
}
