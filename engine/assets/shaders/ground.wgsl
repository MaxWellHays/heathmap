// Terrain colour from per-class signed distance fields (see src/ground.rs).
// Each class is painted over the previous ones where its distance is negative,
// anti-aliased over about one screen pixel, so edges stay sharp at any zoom.
//
// Heights and normals come from the full-resolution heightmap texture, so lighting,
// contour lines and elevation colours agree whichever terrain LOD mesh is drawn.

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

struct ElevationParams {
    // x: elevation colours on, y: contours on, z: contour interval (m), w: major every n-th line
    settings: vec4<f32>,
    // x, y: elevation range (m), z: colour strength, w: contour strength
    range: vec4<f32>,
    gradient: array<vec4<f32>, 5>,
}

struct GroundParams {
    colors: array<vec4<f32>, 13>,
    sdf_scale: vec4<f32>,
    // x, y: world x/z of height sample (0, 0); z: metres per sample
    height_grid: vec4<f32>,
    // x, y: heightmap size in samples
    height_size: vec4<f32>,
    elevation: ElevationParams,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> ground: GroundParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var sdf0: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var sdf_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var sdf1: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var sdf2: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var height_tex: texture_2d<f32>;

// Texture value (0..1) -> signed distance in metres (negative inside).
fn metres(v: f32) -> f32 {
    return (v * 255.0 - 128.0) / ground.sdf_scale.x;
}

fn paint(color: vec3<f32>, d: f32, layer: vec3<f32>) -> vec3<f32> {
    // Width of one screen pixel in metres at this point; keeps edges one pixel soft.
    let w = max(fwidth(d), 0.01);
    return mix(color, layer, 1.0 - smoothstep(-w, w, d));
}

fn height_sample(c: vec2<i32>) -> f32 {
    let size = vec2<i32>(ground.height_size.xy);
    return textureLoad(height_tex, clamp(c, vec2<i32>(0), size - vec2<i32>(1)), 0).r;
}

// Terrain height at world (x, z), interpolated on the same triangles as the full-detail
// mesh (cells split along their NE–SW diagonal), matching Heightmap::sample on the CPU.
fn height_at(xz: vec2<f32>) -> f32 {
    let g = (xz - ground.height_grid.xy) / ground.height_grid.z;
    let f = clamp(g, vec2<f32>(0.0), ground.height_size.xy - vec2<f32>(1.001));
    let c = vec2<i32>(floor(f));
    let t = f - floor(f);
    let a = height_sample(c);
    let b = height_sample(c + vec2<i32>(1, 0));
    let s = height_sample(c + vec2<i32>(0, 1));
    if t.x + t.y <= 1.0 {
        return a + (b - a) * t.x + (s - a) * t.y;
    }
    let d = height_sample(c + vec2<i32>(1, 1));
    return d + (s - d) * (1.0 - t.x) + (b - d) * (1.0 - t.y);
}

fn normal_at(xz: vec2<f32>) -> vec3<f32> {
    let r = ground.height_grid.z;
    let l = height_at(xz - vec2<f32>(r, 0.0));
    let rr = height_at(xz + vec2<f32>(r, 0.0));
    let u = height_at(xz - vec2<f32>(0.0, r));
    let d = height_at(xz + vec2<f32>(0.0, r));
    return normalize(vec3<f32>(l - rr, 2.0 * r, u - d));
}

// Elevation gradient colour for t in 0..1 (five evenly spaced stops).
fn gradient(t: f32) -> vec3<f32> {
    let x = clamp(t, 0.0, 1.0) * 4.0;
    let i = min(u32(x), 3u);
    return mix(ground.elevation.gradient[i].rgb, ground.elevation.gradient[i + 1u].rgb, x - f32(i));
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

    // Full-resolution height and normal at this pixel.
    let xz = in.world_position.xz;
    let e = height_at(xz);
    let n = normal_at(xz);
    pbr_input.N = n;
    pbr_input.world_normal = n;

    // Elevation colours and contour lines (the v0 map's layers).
    // Derivatives are taken unconditionally: they must stay in uniform control flow.
    let el = ground.elevation;
    let tint = gradient((e - el.range.x) / max(el.range.y - el.range.x, 0.1));
    let interval = max(el.settings.z, 0.1);
    let metres_per_pixel = max(fwidth(e), 1e-4);
    let to_line = abs(fract(e / interval + 0.5) - 0.5) * interval; // metres to the nearest contour
    let index = round(e / interval);
    let major = abs(index - el.settings.w * round(index / el.settings.w)) < 0.5;
    let half_width = select(0.5, 1.2, major); // in pixels
    let line = 1.0 - smoothstep(half_width, half_width + 1.0, to_line / metres_per_pixel);
    if el.settings.x > 0.5 {
        color = mix(color, tint, el.range.z);
    }
    if el.settings.y > 0.5 {
        color = mix(color, tint * 0.45, line * el.range.w);
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
