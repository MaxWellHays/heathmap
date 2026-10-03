// Terrain colour from per-class signed distance fields (see src/ground.rs).
// Each class is painted over the previous ones where its distance is negative,
// anti-aliased over about one screen pixel, so edges stay sharp at any zoom.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::alpha_discard,
}

#ifdef PREPASS_PIPELINE
#import bevy_pbr::{
    prepass_io::{VertexOutput, FragmentOutput},
    pbr_deferred_functions::deferred_output,
}
#else
#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
}
#endif

struct GroundParams {
    colors: array<vec4<f32>, 13>,
    sdf_scale: vec4<f32>,
    // x: elevation colours on, y: contours on, z: contour interval (m), w: major every n-th line
    elevation: vec4<f32>,
    // x, y: elevation range (m), z: colour strength, w: contour strength
    elevation_range: vec4<f32>,
    gradient: array<vec4<f32>, 5>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> ground: GroundParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var sdf0: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var sdf_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var sdf1: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var sdf2: texture_2d<f32>;

// Texture value (0..1) -> signed distance in metres (negative inside).
fn metres(v: f32) -> f32 {
    return (v * 255.0 - 128.0) / ground.sdf_scale.x;
}

fn paint(color: vec3<f32>, d: f32, layer: vec3<f32>) -> vec3<f32> {
    // Width of one screen pixel in metres at this point; keeps edges one pixel soft.
    let w = max(fwidth(d), 0.01);
    return mix(color, layer, 1.0 - smoothstep(-w, w, d));
}

// Elevation gradient colour for t in 0..1 (five evenly spaced stops).
fn gradient(t: f32) -> vec3<f32> {
    let x = clamp(t, 0.0, 1.0) * 4.0;
    let i = min(u32(x), 3u);
    return mix(ground.gradient[i].rgb, ground.gradient[i + 1u].rgb, x - f32(i));
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    let a = textureSample(sdf0, sdf_sampler, in.uv);
    let b = textureSample(sdf1, sdf_sampler, in.uv);
    let c = textureSample(sdf2, sdf_sampler, in.uv);
    var color = ground.colors[0].rgb;
    color = paint(color, metres(a.r), ground.colors[1].rgb);
    color = paint(color, metres(a.g), ground.colors[2].rgb);
    color = paint(color, metres(a.b), ground.colors[3].rgb);
    color = paint(color, metres(a.a), ground.colors[4].rgb);
    color = paint(color, metres(b.r), ground.colors[5].rgb);
    color = paint(color, metres(b.g), ground.colors[6].rgb);
    color = paint(color, metres(b.b), ground.colors[7].rgb);
    color = paint(color, metres(b.a), ground.colors[8].rgb);
    color = paint(color, metres(c.r), ground.colors[9].rgb);
    color = paint(color, metres(c.g), ground.colors[10].rgb);
    color = paint(color, metres(c.b), ground.colors[11].rgb);
    color = paint(color, metres(c.a), ground.colors[12].rgb);

    // Elevation colours and contour lines (the v0 map's layers), from the surface height.
    // Derivatives are taken unconditionally: they must stay in uniform control flow.
    let e = in.world_position.y;
    let t = (e - ground.elevation_range.x) / (ground.elevation_range.y - ground.elevation_range.x);
    let tint = gradient(t);
    let interval = max(ground.elevation.z, 0.1);
    let metres_per_pixel = max(fwidth(e), 1e-4);
    let to_line = abs(fract(e / interval + 0.5) - 0.5) * interval; // metres to the nearest contour
    let index = round(e / interval);
    let major = abs(index - ground.elevation.w * round(index / ground.elevation.w)) < 0.5;
    let half_width = select(0.5, 1.2, major); // in pixels
    let line = 1.0 - smoothstep(half_width, half_width + 1.0, to_line / metres_per_pixel);
    if ground.elevation.x > 0.5 {
        color = mix(color, tint, ground.elevation_range.z);
    }
    if ground.elevation.y > 0.5 {
        color = mix(color, tint * 0.45, line * ground.elevation_range.w);
    }
    pbr_input.material.base_color = vec4<f32>(color, 1.0);

#ifdef PREPASS_PIPELINE
    let out = deferred_output(in, pbr_input);
#else
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
#endif
    return out;
}
