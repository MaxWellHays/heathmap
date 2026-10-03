//! The side panel (egui): camera modes, layer toggles, sun position and help,
//! styled like the v0 web map's panel.

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow};
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};

use crate::buildings::Buildings;
use crate::camera::{CameraMode, ModeRequest, cursor_captured};
use crate::lines::Lines;
use crate::trees::Trees;

/// What is drawn and where the sun is. Changed from the panel or keys (B, T).
#[derive(Resource)]
pub struct LayerSettings {
    pub buildings: bool,
    pub trees: bool,
    pub roads: bool,
    pub shadows: bool,
    /// Direction the sunlight comes from, degrees clockwise from north.
    pub sun_azimuth: f32,
    /// Height of the sun above the horizon, degrees.
    pub sun_elevation: f32,
}

impl Default for LayerSettings {
    fn default() -> Self {
        Self { buildings: true, trees: true, roads: true, shadows: true, sun_azimuth: 225.0, sun_elevation: 35.0 }
    }
}

#[derive(Component)]
pub struct Sun;

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(EguiPlugin::default())
            .init_resource::<LayerSettings>()
            .add_systems(EguiPrimaryContextPass, panel)
            .add_systems(Update, (layer_keys, apply_layers).chain());
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

const ACCENT: egui::Color32 = egui::Color32::from_rgb(47, 107, 47);
const MUTED: egui::Color32 = egui::Color32::from_rgb(95, 107, 95);
const COMPASS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];

#[allow(clippy::too_many_arguments)]
fn panel(
    mut contexts: EguiContexts,
    diagnostics: Res<DiagnosticsStore>,
    mode: Res<State<CameraMode>>,
    cursor: Query<&CursorOptions, With<PrimaryWindow>>,
    mut request: ResMut<ModeRequest>,
    mut layers: ResMut<LayerSettings>,
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
        .default_width(230.0)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Hampstead Heath").color(MUTED));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(format!("{fps:.0} fps")).color(MUTED).small());
                });
            });
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
            ui.checkbox(&mut layers.shadows, "Shadows");
            ui.separator();

            ui.label(egui::RichText::new("SUN").color(MUTED).small());
            ui.add(
                egui::Slider::new(&mut layers.sun_azimuth, 0.0..=359.0)
                    .text("from")
                    .custom_formatter(move |v, _| format!("{} {v:.0}°", COMPASS[((v / 45.0).round() as usize) % 8])),
            );
            ui.add(egui::Slider::new(&mut layers.sun_elevation, 3.0..=85.0).text("height").suffix("°"));
            ui.separator();

            egui::CollapsingHeader::new(egui::RichText::new("Controls").color(MUTED)).default_open(false).show(ui, |ui| {
                let help = match current {
                    CameraMode::Map => "Drag: move the point you grabbed\nRight drag / Ctrl+drag: orbit around it\nScroll: zoom towards the cursor",
                    CameraMode::Walk => "WASD: walk, Shift: run (×4.5)\nMouse: look around\nEsc: release the mouse",
                    CameraMode::ThirdPerson => "WASD: walk, Shift: run (×4.5)\nMouse: orbit the camera\nEsc: release the mouse",
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
    )>,
    mut sun: Query<(&mut DirectionalLight, &mut Transform), With<Sun>>,
    added: Query<(), Or<(Added<Buildings>, Added<Trees>, Added<Lines>)>>,
) {
    // Re-apply when settings change or when the layers appear (after level load).
    if !layers.is_changed() && added.is_empty() {
        return;
    }
    let vis = |on: bool| if on { Visibility::Inherited } else { Visibility::Hidden };
    roots.p0().iter_mut().for_each(|mut v| *v = vis(layers.buildings));
    roots.p1().iter_mut().for_each(|mut v| *v = vis(layers.trees));
    roots.p2().iter_mut().for_each(|mut v| *v = vis(layers.roads));
    if let Ok((mut light, mut transform)) = sun.single_mut() {
        light.shadow_maps_enabled = layers.shadows;
        let (az, el) = (layers.sun_azimuth.to_radians(), layers.sun_elevation.to_radians());
        // Towards the sun: x = east, z = south (north is −z).
        let to_sun = Vec3::new(az.sin() * el.cos(), el.sin(), -az.cos() * el.cos());
        *transform = Transform::default().looking_to(-to_sun, Vec3::Y);
    }
}
