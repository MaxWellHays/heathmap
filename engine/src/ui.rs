//! The side panel (egui): camera modes, layer toggles, sun position and help,
//! styled like the v0 web map's panel.

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow};
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};

use crate::bookmarks::Bookmarks;
use crate::buildings::Buildings;
use crate::ground::ElevationSettings;
use crate::camera::{CameraMode, ModeRequest, cursor_captured, keyboard_free};
use crate::import::{ImportRequest, ImportStatus};
use crate::level::{LevelState, SpawnSequence};
use crate::lines::Lines;
use crate::props::{Barriers, Props};
use crate::runs::{Race, RunPlayback, Runs, run_color};
use crate::trees::Trees;

/// What is drawn and where the sun is. Changed from the panel or keys (B, T), or over the
/// Bevy Remote Protocol (resource `heathmap_engine::ui::LayerSettings`).
#[derive(Resource, Reflect)]
#[reflect(Resource)]
pub struct LayerSettings {
    pub buildings: bool,
    pub trees: bool,
    pub roads: bool,
    pub props: bool,
    pub barriers: bool,
    pub shadows: bool,
    /// Direction the sunlight comes from, degrees clockwise from north.
    pub sun_azimuth: f32,
    /// Height of the sun above the horizon, degrees.
    pub sun_elevation: f32,
}

impl Default for LayerSettings {
    fn default() -> Self {
        Self { buildings: true, trees: true, roads: true, props: true, barriers: true, shadows: true, sun_azimuth: 225.0, sun_elevation: 35.0 }
    }
}

#[derive(Component)]
pub struct Sun;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<LayerSettings>()
            .register_type::<LayerSettings>()
            .add_systems(EguiPrimaryContextPass, panel)
            .add_systems(Update, (layer_keys.run_if(keyboard_free), apply_layers).chain());
    }
}

fn style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::light();
    visuals.window_corner_radius = egui::CornerRadius::same(8);
    visuals.window_fill = egui::Color32::from_rgba_unmultiplied(255, 255, 255, 240);
    visuals.selection.bg_fill = egui::Color32::from_rgb(47, 107, 47);
    visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE); // text on selected buttons
    ctx.set_visuals(visuals);
}

enum BookmarkAction {
    Goto(usize),
    Move(usize, usize),
    Delete(usize),
}

const ACCENT: egui::Color32 = egui::Color32::from_rgb(47, 107, 47);
const MUTED: egui::Color32 = egui::Color32::from_rgb(95, 107, 95);
const COMPASS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];

#[allow(clippy::too_many_arguments)]
fn panel(
    mut contexts: EguiContexts,
    diagnostics: Res<DiagnosticsStore>,
    mode: Res<State<CameraMode>>,
    (level_state, spawning): (Res<State<LevelState>>, Res<SpawnSequence>),
    cursor: Query<&CursorOptions, With<PrimaryWindow>>,
    mut request: ResMut<ModeRequest>,
    mut layers: ResMut<LayerSettings>,
    mut elevation: ResMut<ElevationSettings>,
    mut bookmarks: ResMut<Bookmarks>,
    mut bookmark_name: Local<String>,
    runs: Res<Runs>,
    mut playback: ResMut<RunPlayback>,
    mut race: ResMut<Race>,
    (mut import, import_status): (ResMut<ImportRequest>, Res<ImportStatus>),
    mut confirm_forget: Local<bool>,
    mut styled: Local<bool>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    // The egui context only exists once rendering starts, so style it on the first frame.
    if !*styled {
        style(ctx);
        *styled = true;
    }
    let fps = diagnostics.get(&FrameTimeDiagnosticsPlugin::FPS).and_then(|d| d.smoothed()).unwrap_or(0.0);
    let current = *mode.get();
    let captured = cursor.single().is_ok_and(cursor_captured);

    egui::Window::new(egui::RichText::new("heathmap").color(ACCENT).strong().size(18.0))
        .anchor(egui::Align2::LEFT_TOP, [10.0, 10.0])
        .resizable(false)
        .default_width(260.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Hampstead Heath").color(MUTED));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(format!("{fps:.0} fps")).color(MUTED).small());
                });
            });
            if *level_state.get() != LevelState::Ready || !spawning.finished() {
                let what = if *level_state.get() == LevelState::Ready { "Loading buildings and roads…" } else { "Loading map data…" };
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new(what).color(MUTED));
                });
            }
            ui.separator();

            ui.label(egui::RichText::new("VIEW").color(MUTED).small());
            ui.horizontal_wrapped(|ui| {
                for (i, m) in CameraMode::ALL.into_iter().enumerate() {
                    let text = format!("{} {}", i + 1, m.label());
                    if ui.selectable_label(current == m, text).clicked() {
                        request.0 = Some(m);
                    }
                }
            });
            if current.captures_cursor() {
                let hint = if captured { "Esc releases the mouse" } else { "Click the view to capture the mouse" };
                ui.label(egui::RichText::new(hint).color(MUTED).small().italics());
            }
            ui.separator();

            ui.label(egui::RichText::new("LAYERS").color(MUTED).small());
            ui.checkbox(&mut layers.buildings, "Buildings  (B)");
            ui.checkbox(&mut layers.trees, "Trees  (T)");
            ui.checkbox(&mut layers.roads, "Roads and paths");
            ui.checkbox(&mut layers.barriers, "Fences, walls and hedges");
            ui.checkbox(&mut layers.props, "Benches, lamps, signs…");
            ui.checkbox(&mut layers.shadows, "Shadows");
            // Change detection on the settings resource should only fire on real edits.
            let mut e = elevation.clone();
            ui.checkbox(&mut e.colors, "Elevation colours");
            if e.colors {
                ui.add(egui::Slider::new(&mut e.color_strength, 0.1..=1.0).text("strength"));
                ui.checkbox(&mut e.auto_range, "Range from visible heights");
            }
            ui.checkbox(&mut e.contours, "Contours");
            if e.contours {
                ui.add(egui::Slider::new(&mut e.contour_interval, 1.0..=10.0).step_by(1.0).text("every").suffix(" m"));
                ui.add(egui::Slider::new(&mut e.contour_strength, 0.1..=1.0).text("strength"));
            }
            if e != *elevation {
                *elevation = e;
            }
            ui.separator();

            ui.label(egui::RichText::new("SUN").color(MUTED).small());
            ui.add(
                egui::Slider::new(&mut layers.sun_azimuth, 0.0..=359.0)
                    .text("from")
                    .custom_formatter(move |v, _| format!("{} {v:.0}°", COMPASS[((v / 45.0).round() as usize) % 8])),
            );
            ui.add(egui::Slider::new(&mut layers.sun_elevation, 3.0..=85.0).text("height").suffix("°"));
            ui.separator();

            let title = if runs.0.is_empty() { "MY RUNS".to_string() } else { format!("MY RUNS ({})", runs.0.len()) };
            ui.label(egui::RichText::new(title).color(MUTED).small());
            ui.horizontal(|ui| {
                if ui.add_enabled(!import_status.busy, egui::Button::new("Import runs…").small()).clicked() {
                    import.pick_files = true;
                }
                if !runs.0.is_empty() {
                    if !*confirm_forget {
                        *confirm_forget = ui.small_button("Forget my runs").clicked();
                    } else {
                        ui.label(egui::RichText::new("Delete from this device?").small());
                        if ui.small_button("Yes").clicked() {
                            import.forget = true;
                            *confirm_forget = false;
                        }
                        if ui.small_button("No").clicked() {
                            *confirm_forget = false;
                        }
                    }
                }
            });
            if import_status.busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new("Importing…").small().color(MUTED));
                });
            } else if let Some(msg) = &import_status.message {
                ui.label(egui::RichText::new(msg).small().color(MUTED));
            }
            if runs.0.is_empty() {
                ui.label(
                    egui::RichText::new(
                        "Import your Strava export (the .zip from Settings > My Account > Download your data) or GPX / TCX / \
                         FIT files, or drop them on the window. They stay on this device only.",
                    )
                    .small()
                    .italics()
                    .color(MUTED),
                );
                ui.separator();
            } else {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut playback.show_routes, "Routes");
                    ui.checkbox(&mut playback.show_ghosts, "Ghost runners");
                });
                ui.horizontal(|ui| {
                    let label = if playback.playing { "Pause" } else { "Play" };
                    if ui.small_button(label).clicked() {
                        playback.playing = !playback.playing;
                    }
                    if ui.small_button("Restart").clicked() {
                        playback.time = 0.0;
                    }
                    let t = playback.time as u32;
                    ui.label(egui::RichText::new(format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)).small().color(MUTED));
                });
                ui.add(egui::Slider::new(&mut playback.speed, 1.0..=60.0).logarithmic(true).text("speed").suffix("×"));
                // Race ghosts for the routes you're running, picked automatically.
                ui.checkbox(&mut race.enabled, "Race ghosts on my routes (G)");
                if race.enabled && race.ghosts.is_empty() {
                    let hint = if race.trail_length() < 200.0 {
                        "Run one of your routes in Walk or Third person"
                    } else {
                        "No run of yours followed your last stretch"
                    };
                    ui.label(egui::RichText::new(hint).small().italics().color(MUTED));
                }
                for g in &race.ghosts {
                    let run = &runs.0[g.run];
                    let status = if g.finished {
                        "finished".to_string()
                    } else if g.gap_s >= 0.0 {
                        format!("ahead by {:.0} s ({:.0} m)", g.gap_s, g.gap_m.abs())
                    } else {
                        format!("behind by {:.0} s ({:.0} m)", -g.gap_s, g.gap_m.abs())
                    };
                    let swatch = egui::RichText::new("■").color(run_color32(g.run));
                    ui.horizontal(|ui| {
                        ui.label(swatch);
                        ui.label(egui::RichText::new(format!("{} · {:.1} km: {status}", run.date(), run.length_km())).small());
                    });
                }
                ui.separator();
            }

            egui::CollapsingHeader::new(egui::RichText::new("Bookmarks").color(MUTED)).default_open(false).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let field = ui.add(egui::TextEdit::singleline(&mut *bookmark_name).hint_text("name").desired_width(120.0));
                    // Enter in the field saves too (egui drops focus on Enter).
                    let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button("Save view").clicked() || entered {
                        bookmarks.save = Some(std::mem::take(&mut *bookmark_name));
                    }
                });
                let count = bookmarks.list.len();
                let mut action = None;
                for (i, b) in bookmarks.list.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("Go").clicked() {
                            action = Some(BookmarkAction::Goto(i));
                        }
                        // Plain-text labels: the default font has no glyphs for arrows or ✕.
                        if ui.add_enabled(i > 0, egui::Button::new("Up").small()).clicked() {
                            action = Some(BookmarkAction::Move(i, i - 1));
                        }
                        if ui.add_enabled(i + 1 < count, egui::Button::new("Down").small()).clicked() {
                            action = Some(BookmarkAction::Move(i, i + 1));
                        }
                        if ui.small_button("Delete").clicked() {
                            action = Some(BookmarkAction::Delete(i));
                        }
                        ui.label(egui::RichText::new(format!("{}. {}", i + 1, b.name)).small());
                    });
                }
                match action {
                    Some(BookmarkAction::Goto(i)) => bookmarks.goto = Some(i),
                    Some(BookmarkAction::Delete(i)) => bookmarks.delete = Some(i),
                    Some(BookmarkAction::Move(from, to)) => bookmarks.move_to = Some((from, to)),
                    None => {}
                }
            });

            egui::CollapsingHeader::new(egui::RichText::new("Controls").color(MUTED)).default_open(false).show(ui, |ui| {
                let help = match current {
                    CameraMode::Map => "Drag: move the point you grabbed\nRight drag / Ctrl+drag: orbit around it\nScroll: zoom towards the cursor\nWASD / arrows: glide, Shift: ×4.5\nQ/E: turn, R/F: tilt",
                    CameraMode::Walk => "WASD: run at your pace, Shift: ×4.5, Space: jump\nG: race ghosts on your routes on/off\nMouse: look around, Esc: release the mouse",
                    CameraMode::ThirdPerson => "WASD: run at your pace, Shift: ×4.5, Space: jump\nG: race ghosts on your routes on/off\nMouse: orbit the camera, scroll: zoom\nEsc: release the mouse",
                    CameraMode::Fly => "WASD: fly, Space/C: up/down\nShift: ×4.5, scroll: speed\nMouse: look, Esc: release the mouse",
                };
                ui.label(egui::RichText::new(help).small());
                ui.label(egui::RichText::new("Keys 1–4 switch views").small().color(MUTED));
            });
        });
    Ok(())
}

fn layer_keys(keys: Res<ButtonInput<KeyCode>>, mut layers: ResMut<LayerSettings>) {
    if keys.just_pressed(KeyCode::KeyB) {
        layers.buildings = !layers.buildings;
    }
    if keys.just_pressed(KeyCode::KeyT) {
        layers.trees = !layers.trees;
    }
}

#[allow(clippy::type_complexity)]
fn apply_layers(
    layers: Res<LayerSettings>,
    mut roots: ParamSet<(
        Query<&mut Visibility, With<Buildings>>,
        Query<&mut Visibility, With<Trees>>,
        Query<&mut Visibility, With<Lines>>,
        Query<&mut Visibility, With<Props>>,
        Query<&mut Visibility, With<Barriers>>,
    )>,
    mut sun: Query<(&mut DirectionalLight, &mut Transform), With<Sun>>,
    added: Query<(), Or<(Added<Buildings>, Added<Trees>, Added<Lines>, Added<Props>, Added<Barriers>)>>,
) {
    // Re-apply when settings change or when the layers appear (after level load).
    if !layers.is_changed() && added.is_empty() {
        return;
    }
    let vis = |on: bool| if on { Visibility::Inherited } else { Visibility::Hidden };
    roots.p0().iter_mut().for_each(|mut v| *v = vis(layers.buildings));
    roots.p1().iter_mut().for_each(|mut v| *v = vis(layers.trees));
    roots.p2().iter_mut().for_each(|mut v| *v = vis(layers.roads));
    roots.p3().iter_mut().for_each(|mut v| *v = vis(layers.props));
    roots.p4().iter_mut().for_each(|mut v| *v = vis(layers.barriers));
    if let Ok((mut light, mut transform)) = sun.single_mut() {
        light.shadow_maps_enabled = layers.shadows;
        let (az, el) = (layers.sun_azimuth.to_radians(), layers.sun_elevation.to_radians());
        // Towards the sun: x = east, z = south (north is −z).
        let to_sun = Vec3::new(az.sin() * el.cos(), el.sin(), -az.cos() * el.cos());
        *transform = Transform::default().looking_to(-to_sun, Vec3::Y);
    }
}

fn run_color32(i: usize) -> egui::Color32 {
    let [r, g, b, _] = run_color(i).to_srgba().to_u8_array();
    egui::Color32::from_rgb(r, g, b)
}
