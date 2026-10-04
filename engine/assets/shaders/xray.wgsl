// Ghost runners seen through whatever hides them (trees, buildings, hills): a second copy
// of the figure drawn only where it is occluded (depth test inverted, see src/avatar.rs),
// as a translucent silhouette with a bright rim like an outline.
//
// The depth used for that test is pulled towards the camera, so the figure's own limbs
// (closer than PULL) don't count as occluders and only real obstacles show the silhouette.

#import bevy_pbr::{
    mesh_functions,
    skinning,
    forward_io::{Vertex, VertexOutput},
    mesh_view_bindings::view,
    view_transformations::{position_world_to_clip, position_world_to_view, position_view_to_clip},
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> color: vec4<f32>;

const PULL: f32 = 0.8; // metres

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
#ifdef SKINNED
    let world_from_local = skinning::skin_model(vertex.joint_indices, vertex.joint_weights, vertex.instance_index);
#else
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
#endif
#ifdef VERTEX_NORMALS
#ifdef SKINNED
    out.world_normal = skinning::skin_normals(world_from_local, vertex.normal);
#else
    out.world_normal = mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex.instance_index);
#endif
#endif
    out.world_position = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0));
    let clip = position_world_to_clip(out.world_position.xyz);
    // Same place on screen, depth of a point PULL metres nearer (but in front of the near plane).
    var view_pos = position_world_to_view(out.world_position.xyz);
    view_pos.z = min(view_pos.z + PULL, -0.25);
    let pulled = position_view_to_clip(view_pos);
    out.position = vec4<f32>(clip.xy, pulled.z / pulled.w * clip.w, clip.w);
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);
    let v = normalize(view.world_position - in.world_position.xyz);
    let rim = pow(1.0 - abs(dot(n, v)), 2.0);
    return vec4<f32>(color.rgb, color.a * (0.25 + 0.75 * rim));
}
