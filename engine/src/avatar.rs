//! The runner figure for the player and the ghost runners: the rigged Mannequin from
//! Quaternius's Universal Animation Library (CC0, assets/models/runner/), with its idle,
//! walk, jog, sprint and jump clips.
//!
//! Each figure's clip follows its `Gait` (speed and whether it's in the air), crossfading
//! between clips, with playback rate matched to the speed so the feet don't slide. The
//! mannequin is restained per figure: light wood for you, each run's colour for ghosts.

use std::time::Duration;

use bevy::animation::{graph::AnimationNodeIndex, transition::AnimationTransitions};
use bevy::gltf::Gltf;
use bevy::prelude::*;

const MODEL: &str = "models/runner/mannequin.glb";
const CROSSFADE: Duration = Duration::from_millis(250);

/// Animation input for a figure: horizontal speed (m/s) and whether it is in the air.
#[derive(Component, Default)]
pub struct Gait {
    pub speed: f32,
    pub airborne: bool,
}

/// Colours for the mannequin: body and joints.
#[derive(Clone, Copy)]
pub struct FigureColors {
    pub main: Color,
    pub joints: Color,
}

impl Default for FigureColors {
    fn default() -> Self {
        Self { main: Color::srgb(0.86, 0.71, 0.53), joints: Color::srgb(0.62, 0.45, 0.30) }
    }
}

impl FigureColors {
    /// Wood stained towards `tint` (for ghost runners, one colour per run).
    pub fn tinted(tint: Color) -> Self {
        let base = Self::default();
        let mix = |a: Color, t: f32| {
            let (a, b) = (a.to_srgba(), tint.to_srgba());
            Color::srgb(a.red + (b.red - a.red) * t, a.green + (b.green - a.green) * t, a.blue + (b.blue - a.blue) * t)
        };
        Self { main: mix(base.main, 0.65), joints: mix(base.joints, 0.4) }
    }
}

/// The loaded runner model and its animation graph.
#[derive(Resource)]
pub struct RunnerAssets {
    gltf: Handle<Gltf>,
    scene: Handle<WorldAsset>,
    graph: Option<(Handle<AnimationGraph>, Clips)>,
}

/// Graph nodes of the clips the runner uses.
#[derive(Clone, Copy)]
struct Clips {
    idle: AnimationNodeIndex,
    walk: AnimationNodeIndex,
    jog: AnimationNodeIndex,
    sprint: AnimationNodeIndex,
    jump: AnimationNodeIndex,
}

/// On the entity holding a figure's scene: its colours.
#[derive(Component)]
struct Figure(FigureColors);

/// On a figure's animation player: which entity's `Gait` drives it and what it plays.
#[derive(Component)]
struct Animator {
    gait: Entity,
    playing: Option<AnimationNodeIndex>,
}

pub struct AvatarPlugin;

impl Plugin for AvatarPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreStartup, load_assets)
            .add_systems(Update, (build_graph, attach_animators, drive_animators, restain).chain());
    }
}

fn load_assets(mut commands: Commands, assets: Res<AssetServer>) {
    commands.insert_resource(RunnerAssets {
        gltf: assets.load(MODEL),
        scene: assets.load(GltfAssetLabel::Scene(0).from_asset(MODEL)),
        graph: None,
    });
}

/// Adds a runner figure as a child of `parent` (feet at the parent's origin, facing −z).
pub fn spawn_figure(commands: &mut Commands, parent: Entity, colors: FigureColors, assets: &RunnerAssets) {
    // The model faces +z; turn it to face −z like everything else.
    let figure = commands
        .spawn((
            Figure(colors),
            WorldAssetRoot(assets.scene.clone()),
            Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
        ))
        .id();
    commands.entity(parent).add_child(figure);
}

fn build_graph(mut runner: ResMut<RunnerAssets>, gltfs: Res<Assets<Gltf>>, mut graphs: ResMut<Assets<AnimationGraph>>) {
    if runner.graph.is_some() {
        return;
    }
    let Some(gltf) = gltfs.get(&runner.gltf) else { return };
    let clip = |name: &str| gltf.named_animations.get(name).cloned();
    let (Some(idle), Some(walk), Some(jog), Some(sprint), Some(jump)) =
        (clip("Idle_Loop"), clip("Walk_Loop"), clip("Jog_Fwd_Loop"), clip("Sprint_Loop"), clip("Jump_Loop"))
    else {
        error!("Runner model is missing an expected animation");
        return;
    };
    let (graph, nodes) = AnimationGraph::from_clips([idle, walk, jog, sprint, jump]);
    let clips = Clips { idle: nodes[0], walk: nodes[1], jog: nodes[2], sprint: nodes[3], jump: nodes[4] };
    runner.graph = Some((graphs.add(graph), clips));
}

/// Connects each figure's animation player (created when its scene spawns) to the graph
/// and to the `Gait` of the player / ghost it belongs to.
fn attach_animators(
    mut commands: Commands,
    runner: Res<RunnerAssets>,
    players: Query<Entity, (With<AnimationPlayer>, Without<Animator>)>,
    parents: Query<&ChildOf>,
    gaits: Query<(), With<Gait>>,
) {
    let Some((graph, _)) = &runner.graph else { return };
    for entity in &players {
        let Some(gait) = parents.iter_ancestors(entity).find(|e| gaits.contains(*e)) else { continue };
        commands.entity(entity).insert((
            AnimationGraphHandle(graph.clone()),
            AnimationTransitions::new(),
            Animator { gait, playing: None },
        ));
    }
}

/// Picks the clip from the speed (crossfading on change) and matches its playback rate.
fn drive_animators(
    runner: Res<RunnerAssets>,
    gaits: Query<&Gait>,
    mut animators: Query<(&mut Animator, &mut AnimationPlayer, &mut AnimationTransitions)>,
) {
    let Some((_, clips)) = runner.graph else { return };
    for (mut animator, mut player, mut transitions) in &mut animators {
        let Ok(gait) = gaits.get(animator.gait) else { continue };
        // (clip, speed it was animated for in m/s; 0 = play at normal rate)
        let (clip, natural) = if gait.airborne {
            (clips.jump, 0.0)
        } else if gait.speed < 0.3 {
            (clips.idle, 0.0)
        } else if gait.speed < 2.0 {
            (clips.walk, 1.4)
        } else if gait.speed < 5.0 {
            (clips.jog, 3.2)
        } else {
            (clips.sprint, 6.5)
        };
        if animator.playing != Some(clip) {
            transitions.play(&mut player, clip, CROSSFADE).repeat();
            animator.playing = Some(clip);
        }
        if natural > 0.0
            && let Some(active) = player.animation_mut(clip)
        {
            active.set_speed((gait.speed / natural).clamp(0.5, 2.0));
        }
    }
}

/// Replaces the model's materials with the figure's colours (body vs joints told apart by
/// the original colours: an orange body and purple joints).
fn restain(
    mut commands: Commands,
    meshes: Query<(Entity, &MeshMaterial3d<StandardMaterial>), Added<MeshMaterial3d<StandardMaterial>>>,
    parents: Query<&ChildOf>,
    figures: Query<&Figure>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut cache: Local<std::collections::HashMap<(u32, bool), Handle<StandardMaterial>>>,
) {
    for (entity, material) in &meshes {
        let Some(figure) = parents.iter_ancestors(entity).find_map(|e| figures.get(e).ok()) else { continue };
        let Some(original) = materials.get(&material.0) else { continue };
        let base = original.base_color.to_srgba();
        let is_joint = base.blue > base.red;
        let color = if is_joint { figure.0.joints } else { figure.0.main };
        let key = (color.to_srgba().to_u8_array().iter().fold(0u32, |k, b| k * 256 + *b as u32), is_joint);
        let handle = cache
            .entry(key)
            .or_insert_with(|| {
                materials.add(StandardMaterial { base_color: color, perceptual_roughness: 0.45, reflectance: 0.35, ..default() })
            })
            .clone();
        commands.entity(entity).insert(MeshMaterial3d(handle));
    }
}
