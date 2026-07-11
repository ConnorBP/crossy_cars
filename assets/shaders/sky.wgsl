// Skydome vertical-gradient material.
// Driven by two uniform colors bound via AsBindGroup on the SkyMaterial struct.
// `mesh.uv.y` is the sphere latitude: ~0 at the south pole, ~1 at the north pole.
#import bevy_pbr::forward_io::VertexOutput

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> top_color: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var<uniform> bottom_color: vec4<f32>;

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    // Gradient from pale horizon (bottom) up to light blue (top).
    return mix(bottom_color, top_color, mesh.uv.y);
}
