//! Your runs (exported from Strava by data/export_engine.py): each route drawn as a thin
//! coloured line on the ground, and a ghost runner per run replaying it at the pace you
//! actually ran (all starting together; playback speed adjustable from the panel).
//!
//! Racing: in walk / third person, press G (or use the panel) and the past runs that pass
//! close to where you stand, heading the way you face, become race ghosts starting from
//! that very point. They wait until you start moving, then run at their recorded pace;
//! the panel shows how far ahead or behind each one is.
//!
//! Your average moving pace across all runs becomes the default walking speed.

use bevy::prelude::*;

use crate::avatar::{FigureColors, Gait, spawn_figure};
use crate::buildings::Reader;
use crate::level::{BinaryFile, Heightmap, LevelHandles, LevelState};
use crate::camera::{CameraMode, Player, RunnerSpeed, keyboard_free};
use crate::props::Shapes;

const ROUTE_WIDTH: f32 = 0.5;
const ROUTE_LIFT: f32 = 0.16; // just above the highest road surface

pub struct Run {
    #[allow(dead_code)] // kept for linking back to Strava
    pub id: u64,
    /// Start time (Unix seconds, local time).
    pub start: i64,
    /// (position, seconds since start)
    pub points: Vec<(Vec2, f32)>,
    /// Distance covered at each point (m).
    pub along: Vec<f32>,
}

impl Run {
    pub fn duration(&self) -> f32 {
        self.points.last().map_or(0.0, |p| p.1)
    }

    pub fn length_km(&self) -> f32 {
        self.along.last().copied().unwrap_or(0.0) / 1000.0
    }

    /// The run's date as "3 Oct 2026".
    pub fn date(&self) -> String {
        // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
        let z = self.start.div_euclid(86_400) + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        format!("{day} {} {year}", MONTHS[(month - 1) as usize])
    }

    /// Direction of travel around point `i`.
    fn direction_at(&self, i: usize) -> Vec2 {
        let (a, b) = (i.saturating_sub(3), (i + 3).min(self.points.len() - 1));
        (self.points[b].0 - self.points[a].0).normalize_or_zero()
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
            .init_resource::<Race>()
            .add_systems(OnEnter(LevelState::Ready), spawn_runs)
            .add_systems(
                Update,
                (move_ghosts, race_keys.run_if(keyboard_free), start_or_stop_race, update_race, apply_visibility)
                    .chain()
                    .run_if(in_state(LevelState::Ready)),
            );
    }
}

fn decode(bytes: &[u8]) -> Vec<Run> {
    let mut r = Reader { bytes, pos: 0 };
    let count = r.u32();
    (0..count)
        .map(|_| {
            let id = u64::from(r.u32()) | (u64::from(r.u32()) << 32);
            let start = (u64::from(r.u32()) | (u64::from(r.u32()) << 32)) as i64;
            let points: Vec<(Vec2, f32)> = (0..r.u32()).map(|_| (Vec2::new(r.f32(), r.f32()), r.f32())).collect();
            let mut along = Vec::with_capacity(points.len());
            let mut d = 0.0;
            for (k, p) in points.iter().enumerate() {
                if k > 0 {
                    d += p.0.distance(points[k - 1].0);
                }
                along.push(d);
            }
            Run { id, start, points, along }
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
    mut runner_speed: ResMut<RunnerSpeed>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(file) = handles.runs.as_ref().and_then(|h| files.get(h)) else { return };
    runs.0 = decode(&file.0).into_iter().filter(|r| r.points.len() >= 2).collect();
    if runs.0.is_empty() {
        return;
    }
    if let Some(pace) = average_moving_speed(&runs.0) {
        runner_speed.0 = pace;
        info!("Walking speed set to your average running pace: {pace:.2} m/s");
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
        let colors = FigureColors::tinted(run_color(i));
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

/// Average speed over the moving parts of all runs (segments between 1.5 and 7 m/s).
fn average_moving_speed(runs: &[Run]) -> Option<f32> {
    let (mut dist, mut time) = (0.0, 0.0);
    for run in runs {
        for w in run.points.windows(2) {
            let (d, dt) = (w[0].0.distance(w[1].0), w[1].1 - w[0].1);
            if dt > 0.0 && (1.5..7.0).contains(&(d / dt)) {
                dist += d;
                time += dt;
            }
        }
    }
    (time > 60.0).then(|| dist / time)
}

fn apply_visibility(
    playback: Res<RunPlayback>,
    race: Res<Race>,
    mut routes: Query<&mut Visibility, (With<Routes>, Without<Ghosts>)>,
    mut ghosts: Query<&mut Visibility, (With<Ghosts>, Without<Routes>)>,
) {
    if !playback.is_changed() && !race.is_changed() {
        return;
    }
    let vis = |on: bool| if on { Visibility::Inherited } else { Visibility::Hidden };
    routes.iter_mut().for_each(|mut v| *v = vis(playback.show_routes));
    // While racing, only the race ghosts run.
    ghosts.iter_mut().for_each(|mut v| *v = vis(playback.show_ghosts && race.state == RaceState::Off));
}

// ---------------------------------------------------------------------------------------
// Racing your past runs

const MATCH_RADIUS: f32 = 30.0; // m from your position to a run's track
const MATCH_DIRECTION: f32 = 0.35; // cosine: the run must head roughly the way you face
const MAX_RACE_GHOSTS: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum RaceState {
    #[default]
    Off,
    /// Ghosts are placed; the clock starts when you start moving.
    Ready,
    Running,
}

pub struct RaceGhost {
    pub run: usize,
    /// Time into the run at the matched starting point.
    pub t0: f32,
    /// Index of the track point nearest to you (follows you along the track).
    cursor: usize,
    /// Seconds the ghost is ahead (positive) or behind (negative) of you.
    pub gap_s: f32,
    /// Metres the ghost is ahead (positive) or behind (negative) of you, along its route.
    pub gap_m: f32,
    pub finished: bool,
    entity: Entity,
}

#[derive(Resource, Default)]
pub struct Race {
    pub state: RaceState,
    pub elapsed: f32,
    pub ghosts: Vec<RaceGhost>,
    /// Set (by G or the panel) to start a race, or stop the current one.
    pub toggle: bool,
    /// Shown in the panel when no run passes nearby.
    pub message: Option<String>,
}

#[derive(Component)]
struct RaceGhostMarker;

fn race_keys(keys: Res<ButtonInput<KeyCode>>, mode: Res<State<CameraMode>>, mut race: ResMut<Race>) {
    if keys.just_pressed(KeyCode::KeyG) && matches!(mode.get(), CameraMode::Walk | CameraMode::ThirdPerson) {
        race.toggle = true;
    }
}

#[allow(clippy::too_many_arguments)]
fn start_or_stop_race(
    mut commands: Commands,
    mut race: ResMut<Race>,
    runs: Res<Runs>,
    mode: Res<State<CameraMode>>,
    players: Query<(&Transform, &Player)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Leaving walk / third person ends a race.
    let in_runner_mode = matches!(mode.get(), CameraMode::Walk | CameraMode::ThirdPerson);
    if !race.toggle && (in_runner_mode || race.state == RaceState::Off) {
        return;
    }
    race.toggle = false;
    if race.state != RaceState::Off {
        for g in race.ghosts.drain(..) {
            commands.entity(g.entity).despawn();
        }
        race.state = RaceState::Off;
        race.message = None;
        return;
    }
    if !in_runner_mode {
        race.message = Some("Switch to Walk or Third person to race".into());
        return;
    }
    let Ok((tf, player)) = players.single() else { return };
    let here = tf.translation.xz();
    let facing = Vec2::new(-player.heading.sin(), -player.heading.cos());
    // Best matching point of every run: near you, heading your way, with some run left.
    let mut matches: Vec<(f32, usize, usize)> = Vec::new();
    for (ri, run) in runs.0.iter().enumerate() {
        let mut best: Option<(f32, usize)> = None;
        for (i, p) in run.points.iter().enumerate() {
            let d = p.0.distance(here);
            if d > MATCH_RADIUS || run.duration() - p.1 < 60.0 {
                continue;
            }
            let along = run.direction_at(i).dot(facing);
            if along < MATCH_DIRECTION {
                continue;
            }
            let score = d + 10.0 * (1.0 - along);
            if best.is_none_or(|b| score < b.0) {
                best = Some((score, i));
            }
        }
        if let Some((score, i)) = best {
            matches.push((score, ri, i));
        }
    }
    matches.sort_by(|a, b| a.0.total_cmp(&b.0));
    if matches.is_empty() {
        race.message = Some("None of your runs pass here in this direction".into());
        return;
    }
    for &(_, ri, i) in matches.iter().take(MAX_RACE_GHOSTS) {
        let p = runs.0[ri].points[i].0;
        let entity = commands
            .spawn((RaceGhostMarker, Gait::default(), Name::new("Race ghost"), Transform::from_xyz(p.x, tf.translation.y, p.y)))
            .id();
        spawn_figure(&mut commands, entity, FigureColors::tinted(run_color(ri)), &mut meshes, &mut materials);
        race.ghosts.push(RaceGhost { run: ri, t0: runs.0[ri].points[i].1, cursor: i, gap_s: 0.0, gap_m: 0.0, finished: false, entity });
    }
    race.state = RaceState::Ready;
    race.elapsed = 0.0;
    race.message = None;
}

/// Moves the race ghosts and works out how far ahead or behind each one is.
fn update_race(
    time: Res<Time>,
    mut race: ResMut<Race>,
    runs: Res<Runs>,
    hm: Res<Heightmap>,
    players: Query<(&Transform, &Gait), (With<Player>, Without<RaceGhostMarker>)>,
    mut ghost_tfs: Query<(&mut Transform, &mut Gait), With<RaceGhostMarker>>,
) {
    if race.state == RaceState::Off {
        return;
    }
    let Ok((player_tf, player_gait)) = players.single() else { return };
    if race.state == RaceState::Ready && player_gait.speed > 0.5 {
        race.state = RaceState::Running;
    }
    if race.state == RaceState::Running {
        race.elapsed += time.delta_secs();
    }
    let (state, elapsed) = (race.state, race.elapsed);
    let here = player_tf.translation.xz();
    for g in &mut race.ghosts {
        let run = &runs.0[g.run];
        let t = (g.t0 + elapsed).min(run.duration());
        g.finished = t >= run.duration();
        let (p, dir, speed) = run.sample(t);
        if let Ok((mut tf, mut gait)) = ghost_tfs.get_mut(g.entity) {
            tf.translation = Vec3::new(p.x, hm.sample(p.x, p.y), p.y);
            tf.rotation = Quat::from_rotation_y((-dir.x).atan2(-dir.y));
            gait.speed = if state == RaceState::Running && !g.finished { speed } else { 0.0 };
        }
        // Your progress along the ghost's route: the nearest track point near the last one.
        let (lo, hi) = (g.cursor.saturating_sub(30), (g.cursor + 300).min(run.points.len() - 1));
        if let Some(best) = (lo..=hi).min_by(|&a, &b| run.points[a].0.distance(here).total_cmp(&run.points[b].0.distance(here))) {
            g.cursor = best;
        }
        let ghost_index = run.points.partition_point(|q| q.1 <= t).clamp(1, run.points.len() - 1);
        g.gap_s = t - run.points[g.cursor].1;
        g.gap_m = run.along[ghost_index] - run.along[g.cursor];
    }
}
