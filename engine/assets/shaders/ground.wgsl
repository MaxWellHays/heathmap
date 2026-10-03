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
