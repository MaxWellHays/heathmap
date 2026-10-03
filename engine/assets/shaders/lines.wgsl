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

fn gradient(t: f32) -> vec3<f32> {
    let x = clamp(t, 0.0, 1.0) * 4.0;
    let i = min(u32(x), 3u);
    return mix(elevation.gradient[i].rgb, elevation.gradient[i + 1u].rgb, x - f32(i));
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    if elevation.settings.x > 0.5 {
        let t = (in.world_position.y - elevation.range.x) / max(elevation.range.y - elevation.range.x, 0.1);
        let tinted = mix(pbr_input.material.base_color.rgb, gradient(t), elevation.range.z);
        pbr_input.material.base_color = vec4<f32>(tinted, pbr_input.material.base_color.a);
    }

#ifdef PREPASS_PIPELINE
    let out = deferred_output(in, pbr_input);
#else
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
#endif
    return out;
}
