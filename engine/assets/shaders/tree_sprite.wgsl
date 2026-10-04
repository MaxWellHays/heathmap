// Distant trees as sprites (see src/trees.rs): one card per tree that faces the camera,
// shaped and lit here instead of with geometry. All four corners of a card arrive at the
// tree's base; the vertex shader spreads them out:
//   uv   = corner: x in [-1, 1] across the crown, y in [0, 1] from base to top
//   uv_b = (crown radius, tree height)
// From the side a card is the tree's silhouette (trunk and double-cone crown, like the
// near trees); seen from above it tilts towards the camera and turns into a round crown.
//
// The prepass and shadow pass use Bevy's default vertex shader, which leaves every card a
// zero-area triangle, so sprites neither fill the depth prepass nor cast shadows.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    forward_io::{Vertex, VertexOutput, FragmentOutput},
    mesh_functions,
    mesh_view_bindings::view,
    view_transformations::position_world_to_clip,
}

const CROWN_BOTTOM: f32 = 0.25; // fractions of the tree's height, as in trees.rs
const CROWN_MID: f32 = 0.55;
const TRUNK_COLOR: vec3<f32> = vec3<f32>(0.107, 0.06, 0.027); // linear sRGB(0.36, 0.27, 0.18)

struct Card {
    right: vec3<f32>,
    up: vec3<f32>,
    to_camera: vec3<f32>,
    /// 0 seen from the side, 1 seen from straight above.
    from_above: f32,
}

// Card orientation for a tree at `pos`: facing the camera, upright from the side and
// tilting back as the camera looks down from above.
fn card_at(pos: vec3<f32>) -> Card {
    var c: Card;
    c.to_camera = normalize(view.world_position - pos);
    c.from_above = clamp(c.to_camera.y, 0.0, 1.0);
    let flat = normalize(vec3<f32>(c.to_camera.x, 0.0, c.to_camera.z) + vec3<f32>(1e-4, 0.0, 0.0));
    c.right = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), flat));
    c.up = normalize(cross(c.to_camera, c.right));
    return c;
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let base = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0)).xyz;
    let radius = vertex.uv_b.x;
    let height = vertex.uv_b.y;
    // Pivot at the middle of the crown, so the card stays over the tree when it tilts.
    let pivot = base + vec3<f32>(0.0, height * CROWN_MID, 0.0);
    let c = card_at(pivot);
    let corner = vertex.uv;
    let world = pivot + c.right * corner.x * radius + c.up * (corner.y - CROWN_MID) * height;
    out.world_position = vec4<f32>(world, 1.0);
    out.position = position_world_to_clip(world);
    out.world_normal = c.to_camera;
    out.uv = corner;
    out.uv_b = vertex.uv_b;
#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif
    return out;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    let x = in.uv.x;
    let y = in.uv.y;
    let radius = in.uv_b.x;
    let height = in.uv_b.y;
    let c = card_at(in.world_position.xyz);

    // Side silhouette: double cone between CROWN_BOTTOM and the top, widest at CROWN_MID.
    let side_half_width = select((1.0 - y) / (1.0 - CROWN_MID), (y - CROWN_BOTTOM) / (CROWN_MID - CROWN_BOTTOM), y < CROWN_MID);
    // Top view: a disc centred on the pivot (in card units the height is height/radius times longer).
    let dy = (y - CROWN_MID) * height / radius;
    let disc = 1.0 - length(vec2<f32>(x, dy));
    let side = side_half_width - abs(x);
    let inside_crown = mix(side, disc, smoothstep(0.55, 0.9, c.from_above));
    let trunk_half_width = clamp(0.08 * height, 0.15, 0.6) / radius;
    let is_trunk = inside_crown < 0.0 && abs(x) < trunk_half_width && y < CROWN_MID && c.from_above < 0.6;
    if inside_crown < 0.0 && !is_trunk {
        discard;
    }

    var pbr_input = pbr_input_from_standard_material(in, is_front);
    var color = in.color.rgb;
    // A rounded normal across the crown: lit on the sun's side, shaded on the other.
    var n = c.to_camera;
    if is_trunk {
        color = TRUNK_COLOR;
    } else {
        let across = clamp(x, -1.0, 1.0);
        let along = clamp(select((y - CROWN_MID) / (1.0 - CROWN_MID), (y - CROWN_MID) / CROWN_MID, y < CROWN_MID), -1.0, 1.0);
        let facing = sqrt(max(1.0 - across * across - 0.5 * along * along, 0.05));
        n = normalize(c.right * across + c.up * along * 0.7 + c.to_camera * facing);
    }
    pbr_input.material.base_color = vec4<f32>(color, 1.0);
    pbr_input.N = n;
    pbr_input.world_normal = n;

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
