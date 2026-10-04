//! Your runs (imported in the app, see import.rs): each route drawn as a thin
//! coloured line on the ground, and a ghost runner per run replaying it at the pace you
//! actually ran (all starting together; playback speed adjustable from the panel).
//!
//! Racing: in walk / third person your recent path is compared with all your runs; every
//! run that followed the same way (same order, so same direction) gets a race ghost. It
//! starts beside you, from the point of its route nearest to you when it's recognised, and
//! runs on at its recorded pace.
//! Several can race at once; each goes when you leave its route. The panel shows how far
//! ahead or behind each one is. G (or the panel) turns this on and off.
//!
//! Your average moving pace across all runs becomes the default walking speed.

use bevy::prelude::*;

use crate::avatar::{FigureColors, Gait, RunnerAssets, spawn_figure};
use crate::level::{Heightmap, LevelState};
use crate::camera::{CameraMode, Player, RunnerSpeed, keyboard_free};
use crate::props::Shapes;

const ROUTE_WIDTH: f32 = 0.5;
const ROUTE_LIFT: f32 = 0.16; // just above the highest road surface

pub struct Run {
    #[allow(dead_code)] // kept for linking back to Strava
    pub id: u64,
    /// Start time (Unix seconds, UTC).
    pub start: i64,
    /// (position, seconds since start)
    pub points: Vec<(Vec2, f32)>,
    /// Distance covered at each point (m).
    pub along: Vec<f32>,
}

impl Run {
    pub fn new(id: u64, start: i64, points: Vec<(Vec2, f32)>) -> Self {
        let mut along = Vec::with_capacity(points.len());
        let mut d = 0.0;
        for (k, p) in points.iter().enumerate() {
            if k > 0 {
                d += p.0.distance(points[k - 1].0);
            }
            along.push(d);
        }
        Self { id, start, points, along }
    }

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
            .add_systems(
                Update,
                (rebuild_runs, move_ghosts, race_keys.run_if(keyboard_free), track_routes, update_race, apply_visibility)
                    .chain()
                    .run_if(in_state(LevelState::Ready)),
            );
    }
}

/// A distinct, saturated colour per run.
pub fn run_color(i: usize) -> Color {
    Color::hsl((i as f32 * 137.5) % 360.0, 0.85, 0.5)
}

/// (Re)builds the routes and ghost runners whenever your runs change (loaded or imported).
#[allow(clippy::too_many_arguments)]
fn rebuild_runs(
    mut commands: Commands,
    hm: Res<Heightmap>,
    runs: Res<Runs>,
    mut race: ResMut<Race>,
    mut playback: ResMut<RunPlayback>,
    mut runner_speed: ResMut<RunnerSpeed>,
    runner: Res<RunnerAssets>,
    old: Query<Entity, Or<(With<Routes>, With<Ghosts>)>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !runs.is_changed() {
        return;
    }
    for e in &old {
        commands.entity(e).despawn();
    }
    race.clear(&mut commands);
    playback.time = 0.0;
    playback.set_changed(); // re-apply route / ghost visibility to the new entities
    if runs.0.is_empty() {
        runner_speed.0 = RunnerSpeed::default().0;
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
        spawn_figure(&mut commands, ghost, colors, &runner);
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
    ghosts.iter_mut().for_each(|mut v| *v = vis(playback.show_ghosts && !race.active()));
}

// ---------------------------------------------------------------------------------------
// Racing your past runs, picked automatically by route

const TRAIL_STEP: f32 = 5.0; // m between trail points
const TRAIL_LENGTH: f32 = 600.0; // m of your recent path kept for matching
const MIN_MATCH_LENGTH: f32 = 200.0; // m you must run before a route can match
const MATCH_TOLERANCE: f32 = 15.0; // m from your trail to the run's track
const MATCH_FRACTION: f32 = 0.9; // of trail points that must be within tolerance
const MATCH_INTERVAL: f32 = 1.0; // s between matching passes
const OFF_ROUTE: f32 = 40.0; // m away from a ghost's route that counts as leaving it
const OFF_ROUTE_GRACE: f32 = 5.0; // s off route before its ghost goes
const TELEPORT: f32 = 25.0; // m moved in one frame: a jump (bookmark, mode switch), not running
const MAX_RACE_GHOSTS: usize = 8;

/// Your recent path in walk / third person, for recognising which routes you're on.
#[derive(Default)]
struct Trail {
    /// (position, clock time)
    points: Vec<(Vec2, f32)>,
    length: f32,
}

pub struct RaceGhost {
    pub run: usize,
    /// Run time at the point where the race started (beside you, when recognised).
    t0: f32,
    /// Your clock time when you passed that point.
    start_clock: f32,
    /// Index of the track point nearest to you (follows you along the track).
    cursor: usize,
    /// Seconds the ghost is ahead (positive) or behind (negative) of you.
    pub gap_s: f32,
    /// Metres between you and the ghost along its route.
    pub gap_m: f32,
    pub finished: bool,
    off_route_for: f32,
    entity: Entity,
}

#[derive(Resource)]
pub struct Race {
    /// Race ghosts appear automatically when you run one of your routes.
    pub enabled: bool,
    pub ghosts: Vec<RaceGhost>,
    trail: Trail,
    clock: f32,
    since_match: f32,
    last_pos: Option<Vec2>,
}

impl Default for Race {
    fn default() -> Self {
        Self { enabled: true, ghosts: Vec::new(), trail: Trail::default(), clock: 0.0, since_match: 0.0, last_pos: None }
    }
}

impl Race {
    pub fn active(&self) -> bool {
        !self.ghosts.is_empty()
    }

    /// How much of your recent path is available for matching (m).
    pub fn trail_length(&self) -> f32 {
        self.trail.length
    }

    fn clear(&mut self, commands: &mut Commands) {
        for g in self.ghosts.drain(..) {
            commands.entity(g.entity).despawn();
        }
        self.trail = Trail::default();
        self.last_pos = None;
    }
}

#[derive(Component)]
struct RaceGhostMarker;

fn race_keys(keys: Res<ButtonInput<KeyCode>>, mut race: ResMut<Race>) {
    if keys.just_pressed(KeyCode::KeyG) {
        race.enabled = !race.enabled;
    }
}

/// Records your path, recognises the routes you're running and spawns / drops race ghosts.
#[allow(clippy::too_many_arguments)]
fn track_routes(
    mut commands: Commands,
    time: Res<Time>,
    mut race: ResMut<Race>,
    runs: Res<Runs>,
    mode: Res<State<CameraMode>>,
    players: Query<&Transform, With<Player>>,
    runner: Res<RunnerAssets>,
) {
    let running = race.enabled && matches!(mode.get(), CameraMode::Walk | CameraMode::ThirdPerson);
    let Ok(tf) = players.single() else { return };
    if !running || runs.0.is_empty() {
        if race.last_pos.is_some() || race.active() {
            race.clear(&mut commands);
        }
        return;
    }
    let dt = time.delta_secs();
    race.clock += dt;
    let (clock, here) = (race.clock, tf.translation.xz());
    if race.last_pos.is_some_and(|p| p.distance(here) > TELEPORT) {
        race.clear(&mut commands);
    }
    race.last_pos = Some(here);

    // Extend the trail every few metres; drop its oldest part beyond the kept length.
    let trail = &mut race.trail;
    match trail.points.last() {
        Some(&(last, _)) if last.distance(here) < TRAIL_STEP => {}
        last => {
            if let Some(&(last, _)) = last {
                trail.length += last.distance(here);
            }
            trail.points.push((here, clock));
            while trail.length > TRAIL_LENGTH && trail.points.len() > 2 {
                trail.length -= trail.points[0].0.distance(trail.points[1].0);
                trail.points.remove(0);
            }
        }
    }

    // Follow each ghost's route with your position; drop it once you leave the route.
    let mut keep = Vec::new();
    for mut g in std::mem::take(&mut race.ghosts) {
        let run = &runs.0[g.run];
        let (lo, hi) = (g.cursor.saturating_sub(20), (g.cursor + 200).min(run.points.len() - 1));
        let nearest = (lo..=hi).min_by(|&a, &b| run.points[a].0.distance(here).total_cmp(&run.points[b].0.distance(here)));
        if let Some(i) = nearest {
            g.cursor = i;
        }
        let off = run.points[g.cursor].0.distance(here) > OFF_ROUTE || g.cursor + 1 >= run.points.len();
        g.off_route_for = if off { g.off_route_for + dt } else { 0.0 };
        if g.off_route_for > OFF_ROUTE_GRACE {
            commands.entity(g.entity).despawn();
        } else {
            keep.push(g);
        }
    }
    race.ghosts = keep;

    // Every second, look for runs that follow your recent path.
    race.since_match += dt;
    if race.since_match < MATCH_INTERVAL || race.trail.length < MIN_MATCH_LENGTH {
        return;
    }
    race.since_match = 0.0;
    let mut found: Vec<(f32, RaceGhost)> = Vec::new();
    for (ri, run) in runs.0.iter().enumerate() {
        if race.ghosts.iter().any(|g| g.run == ri) {
            continue;
        }
        if let Some((fit, cursor)) = match_trail(run, &race.trail.points) {
            // Start beside you: at the run's point nearest to where you are now.
            found.push((fit, RaceGhost {
                run: ri,
                t0: run.points[cursor].1,
                start_clock: clock,
                cursor,
                gap_s: 0.0,
                gap_m: 0.0,
                finished: false,
                off_route_for: 0.0,
                entity: Entity::PLACEHOLDER,
            }));
        }
    }
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, mut g) in found {
        if race.ghosts.len() >= MAX_RACE_GHOSTS {
            break;
        }
        let p = runs.0[g.run].points[g.cursor].0;
        g.entity = commands
            .spawn((RaceGhostMarker, Gait::default(), Name::new("Race ghost"), Transform::from_xyz(p.x, tf.translation.y, p.y)))
            .id();
        spawn_figure(&mut commands, g.entity, FigureColors::tinted(run_color(g.run)), &runner);
        info!("Racing your run of {} ({:.1} km)", runs.0[g.run].date(), runs.0[g.run].length_km());
        race.ghosts.push(g);
    }
}

/// Whether `run` follows the `trail` (in the same order, so in the same direction).
/// Returns (mean distance, run index where the trail ends, i.e. nearest to you now).
fn match_trail(run: &Run, trail: &[(Vec2, f32)]) -> Option<(f32, usize)> {
    let first = trail[0].0;
    let pts = &run.points;
    // Candidate starts: points closest to the trail's start on each pass of the run near it.
    let mut starts = Vec::new();
    let mut i = 0;
    while i < pts.len() {
        if pts[i].0.distance(first) <= MATCH_TOLERANCE {
            let mut best = i;
            while i < pts.len() && pts[i].0.distance(first) <= MATCH_TOLERANCE {
                if pts[i].0.distance(first) < pts[best].0.distance(first) {
                    best = i;
                }
                i += 1;
            }
            starts.push(best);
        }
        i += 1;
    }
    let max_misses = ((1.0 - MATCH_FRACTION) * trail.len() as f32).floor() as usize;
    let mut best: Option<(f32, usize)> = None;
    for start in starts {
        // Walk the run forward alongside the trail, always to the nearest point ahead.
        let (mut j, mut misses, mut total) = (start, 0, 0.0);
        for &(q, _) in &trail[1..] {
            let hi = (j + 60).min(pts.len() - 1);
            let (k, d) = (j..=hi).map(|k| (k, pts[k].0.distance(q))).min_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
            if d > MATCH_TOLERANCE {
                misses += 1;
                if misses > max_misses {
                    break;
                }
            } else {
                j = k;
            }
            total += d.min(MATCH_TOLERANCE * 2.0);
        }
        let mean = total / (trail.len() - 1) as f32;
        // The run must actually cover the trail, not just touch its start and end.
        let covered = run.along[j] - run.along[start];
        if misses <= max_misses && covered > 0.7 * trail_length(trail) && best.is_none_or(|b| mean < b.0) {
            best = Some((mean, j));
        }
    }
    best
}

fn trail_length(trail: &[(Vec2, f32)]) -> f32 {
    trail.windows(2).map(|w| w[0].0.distance(w[1].0)).sum()
}

/// Moves the race ghosts and works out how far ahead or behind each one is.
fn update_race(
    mut race: ResMut<Race>,
    runs: Res<Runs>,
    hm: Res<Heightmap>,
    mut ghost_tfs: Query<(&mut Transform, &mut Gait), With<RaceGhostMarker>>,
) {
    let clock = race.clock;
    for g in &mut race.ghosts {
        let run = &runs.0[g.run];
        let t = (g.t0 + clock - g.start_clock).min(run.duration());
        g.finished = t >= run.duration();
        let (p, dir, speed) = run.sample(t);
        if let Ok((mut tf, mut gait)) = ghost_tfs.get_mut(g.entity) {
            tf.translation = Vec3::new(p.x, hm.sample(p.x, p.y), p.y);
            tf.rotation = Quat::from_rotation_y((-dir.x).atan2(-dir.y));
            gait.speed = if g.finished { 0.0 } else { speed };
        }
        let ghost_index = run.points.partition_point(|q| q.1 <= t).clamp(1, run.points.len() - 1);
        g.gap_s = t - run.points[g.cursor].1;
        g.gap_m = run.along[ghost_index] - run.along[g.cursor];
    }
}
