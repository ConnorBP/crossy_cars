// Animated water-pond material.
// `base` is a configurable tint; `time` advances the ripple phase each frame.
// Ripples are computed purely in the fragment shader from the plane UVs, so the
// mesh stays a flat quad and no vertex displacement is required.
#import bevy_pbr::forward_io::VertexOutput

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> base: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var<uniform> time: vec4<f32>;

@fragment
fn fragment(mesh: VertexOutput) -> @location(0) vec4<f32> {
    // Fragment-only, low-amplitude UV waves keep the cached pond mesh flat.
    // Combining two directions avoids a high-contrast radial "target" pattern.
    let uv = mesh.uv * 2.0 - 1.0;
    let wave_a = sin((uv.x * 8.0 + uv.y * 5.0) + time.x * 0.55);
    let wave_b = sin((uv.x * -4.0 + uv.y * 9.0) - time.x * 0.38);
    let wave = (wave_a + wave_b) * 0.025;
    let tint = vec3<f32>(wave * 0.45, wave * 0.8, wave);

    // The material is intentionally opaque: ponds are harmless visual ground
    // dressing, not transparent surfaces with sorting/refraction overhead.
    return vec4<f32>(clamp(base.rgb + tint, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
