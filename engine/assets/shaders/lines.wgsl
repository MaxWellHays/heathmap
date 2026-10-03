// Roads and paths: the standard textured material, optionally tinted with the same
// elevation colours as the terrain (see src/lines.rs and src/ground.rs).

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
    settings: vec4<f32>,
    range: vec4<f32>,
    gradient: array<vec4<f32>, 5>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> elevation: ElevationParams;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var<uniform> height_grid: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var<uniform> height_size: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var height_tex: texture_2d<f32>;

fn height_sample(c: vec2<i32>) -> f32 {
    let size = vec2<i32>(height_size.xy);
    return textureLoad(height_tex, clamp(c, vec2<i32>(0), size - vec2<i32>(1)), 0).r;
}

// Terrain height under this point, exactly as in ground.wgsl, so contours on roads line
// up with the contours on the terrain beside them.
fn height_at(xz: vec2<f32>) -> f32 {
    let g = (xz - height_grid.xy) / height_grid.z;
    let f = clamp(g, vec2<f32>(0.0), height_size.xy - vec2<f32>(1.001));
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

fn gradient(t: f32) -> vec3<f32> {
    let x = clamp(t, 0.0, 1.0) * 4.0;
    let i = min(u32(x), 3u);
    return mix(elevation.gradient[i].rgb, elevation.gradient[i + 1u].rgb, x - f32(i));
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    // Same elevation colours and contour lines as the terrain, from the terrain height
    // under the road; contours are drawn over roads so they stay continuous.
    let e = height_at(in.world_position.xz);
    let tint = gradient((e - elevation.range.x) / max(elevation.range.y - elevation.range.x, 0.1));
    let interval = max(elevation.settings.z, 0.1);
    let metres_per_pixel = max(fwidth(e), 1e-4);
    let to_line = abs(fract(e / interval + 0.5) - 0.5) * interval;
    let index = round(e / interval);
    let major = abs(index - elevation.settings.w * round(index / elevation.settings.w)) < 0.5;
    let half_width = select(0.5, 1.2, major);
    let line = 1.0 - smoothstep(half_width, half_width + 1.0, to_line / metres_per_pixel);
    var color = pbr_input.material.base_color.rgb;
    if elevation.settings.x > 0.5 {
        color = mix(color, tint, elevation.range.z);
    }
    if elevation.settings.y > 0.5 {
        color = mix(color, tint * 0.45, line * elevation.range.w);
    }
    pbr_input.material.base_color = vec4<f32>(color, pbr_input.material.base_color.a);

#ifdef PREPASS_PIPELINE
    let out = deferred_output(in, pbr_input);
#else
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
#endif
    return out;
}
