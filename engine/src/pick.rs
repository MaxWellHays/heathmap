//! Scene ray casts against the terrain, trees and buildings, using the level data
//! directly (no mesh copies on the CPU): terrain by marching the heightmap, trees as
//! vertical crown cylinders, buildings as extruded footprints. A coarse grid keeps
//! object tests local. Used for grabbing points with the mouse; later for collisions.

use bevy::prelude::*;
use std::collections::HashMap;

use crate::buildings::BuildingIndex;
use crate::level::{Heightmap, Level, LevelState};
use crate::terrain::rendered_step;

const CELL: f32 = 32.0;

#[derive(Resource, Default)]
pub struct PickGrid {
    trees: HashMap<(i32, i32), Vec<u32>>,
    buildings: HashMap<(i32, i32), Vec<u32>>,
    ready: bool,
}

pub struct PickPlugin;

impl Plugin for PickPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PickGrid>()
            .add_systems(Update, build_grid.run_if(in_state(LevelState::Ready)));
    }
}

fn cell(p: Vec2) -> (i32, i32) {
    ((p.x / CELL).floor() as i32, (p.y / CELL).floor() as i32)
}

fn build_grid(mut grid: ResMut<PickGrid>, level: Res<Level>, index: Res<BuildingIndex>) {
    if grid.ready || index.buildings.is_empty() {
        return;
    }
    for (i, t) in level.trees.iter().enumerate() {
        let r = t.crown_radius.max(1.0);
        for c in cells_in(Vec2::new(t.x - r, t.z - r), Vec2::new(t.x + r, t.z + r)) {
            grid.trees.entry(c).or_default().push(i as u32);
        }
    }
    for (i, b) in index.buildings.iter().enumerate() {
        let (lo, hi) = b.rings[0].iter().fold((Vec2::MAX, Vec2::MIN), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        for c in cells_in(lo, hi) {
            grid.buildings.entry(c).or_default().push(i as u32);
        }
    }
    grid.ready = true;
}

fn cells_in(lo: Vec2, hi: Vec2) -> impl Iterator<Item = (i32, i32)> {
    let (a, b) = (cell(lo), cell(hi));
    (a.0..=b.0).flat_map(move |x| (a.1..=b.1).map(move |z| (x, z)))
}

/// First terrain hit along the ray (distance), by marching with a growing step.
/// Uses full-detail heights; see `rendered_terrain_hit` for what is on screen.
pub fn terrain_hit(ray: Ray3d, hm: &Heightmap, max: f32) -> Option<f32> {
    march(ray, max, |p| hm.sample(p.x, p.z))
}

/// Terrain hit against the LOD meshes currently drawn (the camera is at the ray origin).
pub fn rendered_terrain_hit(ray: Ray3d, hm: &Heightmap, max: f32) -> Option<f32> {
    march(ray, max, |p| hm.sample_step(p.x, p.z, rendered_step(hm, ray.origin, p.x, p.z)))
}

fn march(ray: Ray3d, max: f32, height: impl Fn(Vec3) -> f32) -> Option<f32> {
    let (mut t, mut step) = (0.0, 0.5);
    let mut prev_above = true;
    let mut prev_t = 0.0;
    while t < max {
        let p = ray.get_point(t);
        let above = p.y > height(p);
        if !above && prev_above {
            // Refine between the last point above and this one.
            let (mut lo, mut hi) = (prev_t, t);
            for _ in 0..12 {
                let mid = (lo + hi) / 2.0;
                let q = ray.get_point(mid);
                if q.y > height(q) { lo = mid } else { hi = mid }
            }
            return Some(hi);
        }
        prev_above = above;
        prev_t = t;
        t += step;
        step = (step * 1.04).min(8.0);
    }
    None
}

/// Nearest hit on terrain, trees or buildings.
pub fn raycast(ray: Ray3d, level: &Level, index: &BuildingIndex, grid: &PickGrid) -> Option<Vec3> {
    let hm = &level.heightmap;
    let mut best = rendered_terrain_hit(ray, hm, 30_000.0).unwrap_or(f32::MAX);
    if !grid.ready {
        return (best < f32::MAX).then(|| ray.get_point(best));
    }
    // Visit grid cells along the ray's horizontal projection, up to the terrain hit.
    let o = ray.origin.xz();
    let d = ray.direction.xz();
    let horizontal = d.length();
    let reach = if best < f32::MAX { best * horizontal } else { 6000.0 };
    let mut seen_trees = std::collections::HashSet::new();
    let mut seen_buildings = std::collections::HashSet::new();
    let steps = (reach / (CELL * 0.5)).ceil() as usize + 1;
    let dir2 = d.normalize_or_zero();
    for k in 0..=steps {
        let c = cell(o + dir2 * (k as f32 * CELL * 0.5));
        for dc in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)] {
            let c = (c.0 + dc.0, c.1 + dc.1);
            for &i in grid.trees.get(&c).into_iter().flatten() {
                if seen_trees.insert(i) {
                    let t = &level.trees[i as usize];
                    let ground = hm.sample(t.x, t.z);
                    let h = t.height.max(3.0);
                    if let Some(hit) = ray_cylinder(ray, Vec2::new(t.x, t.z), t.crown_radius.clamp(1.0, h * 0.6), ground + h * 0.25, ground + h) {
                        best = best.min(hit);
                    }
                }
            }
            for &i in grid.buildings.get(&c).into_iter().flatten() {
                if seen_buildings.insert(i) {
                    let b = &index.buildings[i as usize];
                    if let Some(hit) = ray_prism(ray, &b.rings[0], b.ground + b.height) {
                        best = best.min(hit);
                    }
                }
            }
        }
    }
    (best < f32::MAX).then(|| ray.get_point(best))
}

/// Vertical cylinder from y0 to y1 (sides and top cap).
fn ray_cylinder(ray: Ray3d, center: Vec2, r: f32, y0: f32, y1: f32) -> Option<f32> {
    let o = ray.origin.xz() - center;
    let d = ray.direction.xz();
    let a = d.length_squared();
    let mut best: Option<f32> = None;
    if a > 1e-9 {
        let b = o.dot(d);
        let c = o.length_squared() - r * r;
        let disc = b * b - a * c;
        if disc >= 0.0 {
            let t = (-b - disc.sqrt()) / a;
            let y = ray.origin.y + ray.direction.y * t;
            if t > 0.0 && (y0..=y1).contains(&y) {
                best = Some(t);
            }
        }
    }
    if ray.direction.y.abs() > 1e-6 {
        let t = (y1 - ray.origin.y) / ray.direction.y;
        if t > 0.0 && (ray.get_point(t).xz() - center).length_squared() <= r * r {
            best = Some(best.map_or(t, |b| b.min(t)));
        }
    }
    best
}

/// Extruded polygon from the ground up to `top`: top face and walls.
fn ray_prism(ray: Ray3d, ring: &[Vec2], top: f32) -> Option<f32> {
    let mut best: Option<f32> = None;
    if ray.direction.y.abs() > 1e-6 {
        let t = (top - ray.origin.y) / ray.direction.y;
        if t > 0.0 && point_in_polygon(ray.get_point(t).xz(), ring) {
            best = Some(t);
        }
    }
    let o = ray.origin.xz();
    let d = ray.direction.xz();
    for k in 0..ring.len() {
        let (p, q) = (ring[k], ring[(k + 1) % ring.len()]);
        let e = q - p;
        let denom = d.perp_dot(e);
        if denom.abs() < 1e-9 {
            continue;
        }
        let t = (p - o).perp_dot(e) / denom;
        let s = (p - o).perp_dot(d) / denom;
        if t > 0.0 && (0.0..=1.0).contains(&s) && ray.origin.y + ray.direction.y * t <= top {
            best = Some(best.map_or(t, |b| b.min(t)));
        }
    }
    best
}

fn point_in_polygon(p: Vec2, ring: &[Vec2]) -> bool {
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}
